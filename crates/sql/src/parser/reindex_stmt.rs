//! `REINDEX [[schema.]name]`, recognized before sqlparser (whose SQLite
//! dialect does not know the statement). The name is resolved when the
//! statement runs (see `exec/reindex.rs`).

use std::sync::Arc;

use redlinedb_kernel::catalog::SchemaEpoch;

use crate::error::{Error, Result};
use crate::statement::{PreparedKind, PreparedTemplate, ReindexTarget};

use super::templates::template;

pub(crate) fn parse_reindex_template(
    sql: &str,
    schema_epoch: SchemaEpoch,
) -> Result<Option<PreparedTemplate>> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    let keyword = "reindex";
    let Some(head) = trimmed.get(..keyword.len()) else {
        return Ok(None);
    };
    let rest = &trimmed[keyword.len()..];
    if !head.eq_ignore_ascii_case(keyword) || rest.chars().next().is_some_and(is_ident_char) {
        return Ok(None);
    }
    let target = parse_target(rest)?;
    Ok(Some(template(
        trimmed,
        schema_epoch,
        false,
        PreparedKind::ReindexIndexes(target),
    )))
}

fn parse_target(rest: &str) -> Result<ReindexTarget> {
    let mut cursor = rest.trim_start();
    if cursor.is_empty() {
        return Ok(ReindexTarget::All);
    }
    let (first, after) = parse_name(cursor)?;
    cursor = after.trim_start();
    let (schema, name) = match cursor.strip_prefix('.') {
        Some(after_dot) => {
            let after_dot = after_dot.trim_start();
            if after_dot.is_empty() {
                return Err(Error::Parse("incomplete input".to_owned()));
            }
            let (second, after) = parse_name(after_dot)?;
            cursor = after.trim_start();
            (Some(first), second)
        }
        None => (None, first),
    };
    if !cursor.is_empty() {
        return Err(syntax_error(cursor));
    }
    Ok(ReindexTarget::Named {
        schema: schema.map(Arc::from),
        name: Arc::from(name),
    })
}

/// One SQLite name: a bare identifier or one quoted with `"`, `` ` ``,
/// `[...]` or `'` (SQLite's `nm` also takes a string literal).
fn parse_name(input: &str) -> Result<(String, &str)> {
    let mut chars = input.char_indices();
    let Some((_, open)) = chars.next() else {
        return Err(syntax_error(input));
    };
    let close = match open {
        '"' | '`' | '\'' => open,
        '[' => ']',
        c if is_ident_char(c) && !c.is_ascii_digit() => {
            let end = input
                .char_indices()
                .find(|&(_, c)| !is_ident_char(c))
                .map_or(input.len(), |(at, _)| at);
            return Ok((input[..end].to_owned(), &input[end..]));
        }
        _ => return Err(syntax_error(input)),
    };
    let mut name = String::new();
    while let Some((at, c)) = chars.next() {
        if c != close {
            name.push(c);
            continue;
        }
        // A doubled quote is an escaped quote; `[...]` has no escape.
        if open != '[' && input[at + c.len_utf8()..].starts_with(close) {
            chars.next();
            name.push(c);
            continue;
        }
        return Ok((name, &input[at + c.len_utf8()..]));
    }
    Err(Error::Parse(format!("unrecognized token: \"{input}\"")))
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$' || !c.is_ascii()
}

fn syntax_error(at: &str) -> Error {
    let token: String = at
        .chars()
        .take_while(|c| !c.is_whitespace())
        .take(32)
        .collect();
    Error::Parse(format!("near \"{token}\": syntax error"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(sql: &str) -> Option<ReindexTarget> {
        parse_reindex_template(sql, SchemaEpoch(0))
            .expect("parses")
            .map(|template| match template.kind {
                PreparedKind::ReindexIndexes(target) => target,
                other => panic!("unexpected kind {other:?}"),
            })
    }

    fn named(schema: Option<&str>, name: &str) -> Option<ReindexTarget> {
        Some(ReindexTarget::Named {
            schema: schema.map(Arc::from),
            name: Arc::from(name),
        })
    }

    #[test]
    fn reindex_forms() {
        assert_eq!(target("REINDEX"), Some(ReindexTarget::All));
        assert_eq!(target("  reindex ;"), Some(ReindexTarget::All));
        assert_eq!(target("REINDEX t"), named(None, "t"));
        assert_eq!(target("REINDEX main.t1"), named(Some("main"), "t1"));
        assert_eq!(target("REINDEX main . \"a b\""), named(Some("main"), "a b"));
        assert_eq!(target("REINDEX \"x\"\"y\""), named(None, "x\"y"));
        assert_eq!(target("REINDEX [i]"), named(None, "i"));
        assert_eq!(target("REINDEX `i`"), named(None, "i"));
        assert_eq!(target("REINDEX nocase;"), named(None, "nocase"));
        assert_eq!(target("REINDEXES"), None);
        assert_eq!(target("SELECT 1"), None);
    }

    #[test]
    fn reindex_syntax_errors() {
        for sql in [
            "REINDEX t x",
            "REINDEX main.",
            "REINDEX 1t",
            "REINDEX \"t",
            "REINDEX a.b.c",
        ] {
            assert!(
                matches!(
                    parse_reindex_template(sql, SchemaEpoch(0)),
                    Err(Error::Parse(_))
                ),
                "{sql} should not parse"
            );
        }
    }
}
