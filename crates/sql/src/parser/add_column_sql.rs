//! The `CREATE TABLE` text `sqlite_master` shows after `ALTER TABLE ... ADD
//! COLUMN`. SQLite splices the new column's definition in after the last
//! column definition, before any table constraint, so everything else the
//! table was created with (its CHECK and UNIQUE clauses, a comment) stays in
//! the text, and `.schema` and `.dump` reproduce the added column's
//! constraints.

use redlinedb_kernel::catalog::SchemaSnapshot;

/// The `CREATE TABLE` text of the table named `folded`, as `sqlite_master`
/// shows it now, with `column_def` added as its last column.
pub(crate) fn table_sql_after_add(
    schema: &SchemaSnapshot,
    folded: &str,
    column_def: &str,
) -> Option<String> {
    let row = schema
        .sqlite_schema_rows()
        .into_iter()
        .find(|row| &*row.type_name == "table" && row.name.eq_ignore_ascii_case(folded))?;
    with_added_column(&row.sql, column_def)
}

/// The column definition as the ALTER statement spells it: the text after
/// `ADD [COLUMN] [IF NOT EXISTS]`, without a trailing `;`. SQLite keeps that
/// text verbatim; `None` when the statement is not in that plain shape.
pub(crate) fn column_def_text(alter_sql: &str) -> Option<&str> {
    let mut rest = keyword(alter_sql, "ALTER")?;
    rest = keyword(rest, "TABLE")?;
    rest = qualified_name(rest)?;
    rest = keyword(rest, "ADD")?;
    rest = keyword(rest, "COLUMN").unwrap_or(rest);
    if let Some(after) = keyword(rest, "IF")
        .and_then(|r| keyword(r, "NOT"))
        .and_then(|r| keyword(r, "EXISTS"))
    {
        rest = after;
    }
    let def = rest.trim().trim_end_matches(';').trim_end();
    (!def.is_empty()).then_some(def)
}

/// `text` after the keyword `word` (any case) and the whitespace before it.
fn keyword<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let text = text.trim_start();
    let head = text.get(..word.len())?;
    let after = &text[word.len()..];
    let joined = after
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$');
    (head.eq_ignore_ascii_case(word) && !joined).then_some(after)
}

/// `text` after a table name: one or two `.`-separated parts, each bare or
/// quoted with `"`, `` ` `` or `[...]`.
fn qualified_name(text: &str) -> Option<&str> {
    let mut rest = name_part(text.trim_start())?;
    if let Some(after_dot) = rest.trim_start().strip_prefix('.') {
        rest = name_part(after_dot.trim_start())?;
    }
    Some(rest)
}

fn name_part(text: &str) -> Option<&str> {
    let quote = text.chars().next()?;
    let close = match quote {
        '"' | '`' => quote,
        '[' => ']',
        _ => {
            let end = text
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
                .unwrap_or(text.len());
            return (end > 0).then(|| &text[end..]);
        }
    };
    let mut chars = text.char_indices().skip(1).peekable();
    while let Some((index, c)) = chars.next() {
        if c == close {
            if close != ']' && chars.peek().is_some_and(|&(_, next)| next == close) {
                chars.next();
                continue;
            }
            return Some(&text[index + c.len_utf8()..]);
        }
    }
    None
}

/// `table_sql` with `column_def` added as the last column, or `None` when
/// the column list cannot be found.
pub(crate) fn with_added_column(table_sql: &str, column_def: &str) -> Option<String> {
    // No trimming: SQLite splices at the exact offset, and text before a
    // closing `)` may end in a `--` comment that needs its newline.
    let close = insertion_point(table_sql)?;
    let head = &table_sql[..close];
    let mut out = String::with_capacity(table_sql.len() + column_def.len() + 2);
    out.push_str(head);
    out.push_str(", ");
    out.push_str(column_def);
    out.push_str(&table_sql[close..]);
    Some(out)
}

/// Byte index where the new column goes: the comma before the first table
/// constraint, or the `)` that closes the column list. Quoted names, string
/// literals and comments are skipped.
fn insertion_point(sql: &str) -> Option<usize> {
    let bytes = sql.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"' | b'`') => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == quote {
                        if bytes.get(i + 1) == Some(&quote) {
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    i += 1;
                }
            }
            b'[' => {
                while i < bytes.len() && bytes[i] != b']' {
                    i += 1;
                }
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 1;
            }
            b'(' => depth += 1,
            b',' if depth == 1 && starts_table_constraint(&sql[i + 1..]) => return Some(i),
            b')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Whether the column-list entry at the start of `rest` is a table
/// constraint rather than a column definition.
fn starts_table_constraint(rest: &str) -> bool {
    let word: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    ["CONSTRAINT", "PRIMARY", "UNIQUE", "CHECK", "FOREIGN"]
        .iter()
        .any(|keyword| word.eq_ignore_ascii_case(keyword))
}

#[cfg(test)]
mod tests {
    use super::{column_def_text, with_added_column};

    #[test]
    fn the_column_definition_is_taken_as_written() {
        assert_eq!(
            column_def_text("ALTER TABLE t ADD COLUMN b INT CHECK(b<9);"),
            Some("b INT CHECK(b<9)")
        );
        assert_eq!(
            column_def_text("alter table main.\"t x\" add if not exists c"),
            Some("c")
        );
        assert_eq!(
            column_def_text("ALTER TABLE [add] ADD d TEXT"),
            Some("d TEXT")
        );
        assert_eq!(column_def_text("ALTER TABLE t RENAME TO u"), None);
    }

    #[test]
    fn the_column_goes_before_the_list_closes() {
        assert_eq!(
            with_added_column("CREATE TABLE t(a INTEGER)", "b INTEGER CHECK (b >= 0)").as_deref(),
            Some("CREATE TABLE t(a INTEGER, b INTEGER CHECK (b >= 0))")
        );
        assert_eq!(
            with_added_column(
                "CREATE TABLE \"we(ird\" (a CHECK (a > 0), c, UNIQUE(c)) STRICT",
                "b INT"
            )
            .as_deref(),
            Some("CREATE TABLE \"we(ird\" (a CHECK (a > 0), c, b INT, UNIQUE(c)) STRICT")
        );
        assert_eq!(
            with_added_column("CREATE TABLE t(a DEFAULT ')' -- )\n)", "b").as_deref(),
            Some("CREATE TABLE t(a DEFAULT ')' -- )\n, b)")
        );
        assert_eq!(with_added_column("CREATE TABLE t", "b"), None);
    }
}
