//! Declared column collations and the collation of index keys (workplan
//! Q5-10).
//!
//! A column declared `COLLATE NOCASE` or `COLLATE RTRIM` compares with that
//! collation wherever SQLite would use the column's collation, and an index
//! key on such a column inherits it unless the index names its own. The
//! B-tree compares key bytes, so a key collation is applied by normalizing
//! the text before it is encoded: NOCASE folds ASCII letters to lower case,
//! RTRIM drops trailing spaces. Only those two (and BINARY) can be index key
//! collations; any other would order keys in a way the bytes cannot.
//!
//! Catalog format 8 stores both the column collation and each key's
//! collation. A format-7 catalog stored neither: a column's collation is
//! read back from its table's CREATE TABLE text, and a key has only the
//! collation its CREATE INDEX text names, which is how the index was built.

use std::borrow::Cow;

use super::key::{IndexKeyDef, IndexKeySource};
use super::schema::{ColumnDef, IndexDef, TableDef};

pub const NOCASE: &str = "NOCASE";
pub const RTRIM: &str = "RTRIM";
pub const BINARY: &str = "BINARY";

/// The collation name the catalog keeps for a `COLLATE` clause: `None` for
/// BINARY (and the Postgres `C` / `POSIX` spellings of it), `NOCASE` or
/// `RTRIM` upper-cased, and any other name unquoted as written.
pub fn canonical_collation(name: &str) -> Option<Box<str>> {
    let name = unquote(name.trim());
    match name.to_ascii_uppercase().as_str() {
        "" | "BINARY" | "C" | "POSIX" => None,
        "NOCASE" => Some(Box::from(NOCASE)),
        "RTRIM" => Some(Box::from(RTRIM)),
        _ => Some(Box::from(name)),
    }
}

/// The collation an explicit `COLLATE` on an index key column names, kept
/// as `BINARY` when it names BINARY so it stays distinguishable from "no
/// COLLATE" (which inherits the column's collation).
pub fn explicit_key_collation(name: &str) -> Box<str> {
    canonical_collation(name).unwrap_or_else(|| Box::from(BINARY))
}

/// Whether the B-tree can order keys by `collation`.
pub fn is_index_key_collation(collation: Option<&str>) -> bool {
    matches!(collation, None | Some(BINARY | NOCASE | RTRIM))
}

/// Two key collations order and compare text the same way.
pub fn same_collation(left: Option<&str>, right: Option<&str>) -> bool {
    let binary = |c: Option<&str>| c.is_none_or(|name| name.eq_ignore_ascii_case(BINARY));
    match (binary(left), binary(right)) {
        (true, true) => true,
        (false, false) => left
            .zip(right)
            .is_some_and(|(l, r)| l.eq_ignore_ascii_case(r)),
        _ => false,
    }
}

/// The text an index key stores for `text` under `collation`.
pub fn normalize_key_text<'a>(text: &'a str, collation: Option<&str>) -> Cow<'a, str> {
    match collation {
        Some(NOCASE) if text.bytes().any(|b| b.is_ascii_uppercase()) => {
            Cow::Owned(text.to_ascii_lowercase())
        }
        Some(RTRIM) if text.ends_with(' ') => Cow::Borrowed(text.trim_end_matches(' ')),
        _ => Cow::Borrowed(text),
    }
}

/// The collation each key of a new index on `table` gets: the one its
/// column spec names, else the declared collation of the column it indexes.
/// An expression key has only an explicit collation.
pub fn inherit_key_collations(table: &TableDef, keys: &mut [IndexKeyDef]) {
    inherit_key_collations_from(&table.columns, keys);
}

/// [`inherit_key_collations`] over a column list that is not in a
/// `TableDef` yet.
pub fn inherit_key_collations_from(columns: &[ColumnDef], keys: &mut [IndexKeyDef]) {
    if v4_key_collations_active() {
        return;
    }
    inherit_into(columns, keys);
}

fn inherit_into(columns: &[ColumnDef], keys: &mut [IndexKeyDef]) {
    for key in keys {
        if key.collation.is_some() {
            continue;
        }
        if let IndexKeySource::Column { attnum } = key.source {
            key.collation = columns
                .iter()
                .find(|column| column.ordinal == attnum)
                .and_then(|column| column.collation.clone());
        }
    }
}

/// Refuse a key collation the B-tree cannot order by. Such an index would
/// otherwise be built with byte order and answer lookups and UNIQUE checks
/// by the wrong comparison.
pub fn check_index_key_collations(keys: &[IndexKeyDef]) -> crate::Result<()> {
    if keys
        .iter()
        .all(|key| is_index_key_collation(key.collation.as_deref()))
    {
        Ok(())
    } else {
        Err(crate::Error::UnsupportedDdl(
            "an index key can use only the BINARY, NOCASE or RTRIM collation",
        ))
    }
}

/// Existing databases (workplan Q5-10 step 4).
///
/// An index built before key collations were inherited has keys whose
/// collation is only what its CREATE INDEX text names. When a key of
/// `index` indexes a column that declares NOCASE or RTRIM and the index
/// named no collation for it, this returns the key collations the index
/// should have; `None` means it already has them. Such an index still
/// answers correctly under the collation it was built with (the planner
/// uses it only for BINARY comparisons), but UNIQUE on it compares
/// binary. The open-time upgrade rebuilds it with the returned collations
/// (`Engine::indexes_needing_rebuild`) and fails when two rows then share
/// a key, never keeping one of them. A column collation an index key cannot
/// use is not inherited here: such an index keeps its BINARY keys.
pub fn index_keys_needing_inherited_collation(
    table: &TableDef,
    index: &IndexDef,
) -> Option<Vec<Option<Box<str>>>> {
    let mut keys = index.keys.clone();
    inherit_into(&table.columns, &mut keys);
    for (inherited, built) in keys.iter_mut().zip(&index.keys) {
        if built.collation.is_none() && !is_index_key_collation(inherited.collation.as_deref()) {
            inherited.collation = None;
        }
    }
    let changed = keys
        .iter()
        .zip(&index.keys)
        .any(|(inherited, built)| inherited.collation != built.collation);
    changed.then(|| keys.into_iter().map(|key| key.collation).collect())
}

thread_local! {
    static V4_KEY_COLLATIONS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// How many threads are inside [`with_v4_key_collations_for_tests`]; the
/// DDL path reads this counter before it consults the thread-local.
static V4_KEY_COLLATIONS_ARMED: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Run `f` with this thread creating index keys as RedlineDB 4.x did: with
/// only the collation an index names, never its column's. Test-only: a
/// database written inside `f` is what an upgrade from 4.x must bring in
/// line.
#[doc(hidden)]
pub fn with_v4_key_collations_for_tests<R>(f: impl FnOnce() -> R) -> R {
    use std::sync::atomic::Ordering;
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            V4_KEY_COLLATIONS.with(|flag| flag.set(self.0));
            V4_KEY_COLLATIONS_ARMED.fetch_sub(1, Ordering::SeqCst);
        }
    }
    V4_KEY_COLLATIONS_ARMED.fetch_add(1, Ordering::SeqCst);
    let _reset = Reset(V4_KEY_COLLATIONS.with(|flag| flag.replace(true)));
    f()
}

fn v4_key_collations_active() -> bool {
    V4_KEY_COLLATIONS_ARMED.load(std::sync::atomic::Ordering::Relaxed) != 0
        && V4_KEY_COLLATIONS.with(std::cell::Cell::get)
}

fn unquote(name: &str) -> &str {
    let bytes = name.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if matches!(
            (first, last),
            (b'"', b'"') | (b'`', b'`') | (b'\'', b'\'') | (b'[', b']')
        ) {
            return &name[1..name.len() - 1];
        }
    }
    name
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// A bare word: keyword or identifier.
    Word(String),
    /// A quoted identifier or a string literal, unquoted.
    Quoted(String),
    Open,
    Close,
    Comma,
    Other,
}

fn tokenize(sql: &str) -> Vec<Token> {
    let bytes = sql.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
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
                i += 2;
            }
            b'"' | b'`' | b'\'' | b'[' => {
                let close = if b == b'[' { b']' } else { b };
                let mut value = Vec::new();
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == close {
                        // A doubled quote is one quote inside the name.
                        if close != b']' && bytes.get(i + 1) == Some(&close) {
                            value.push(close);
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    value.push(bytes[i]);
                    i += 1;
                }
                i += 1;
                tokens.push(Token::Quoted(String::from_utf8_lossy(&value).into_owned()));
            }
            b'(' => {
                tokens.push(Token::Open);
                i += 1;
            }
            b')' => {
                tokens.push(Token::Close);
                i += 1;
            }
            b',' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            _ if b.is_ascii_whitespace() => i += 1,
            _ if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80 => {
                let start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric()
                        || bytes[i] == b'_'
                        || bytes[i] == b'$'
                        || bytes[i] >= 0x80)
                {
                    i += 1;
                }
                tokens.push(Token::Word(
                    String::from_utf8_lossy(&bytes[start..i]).into_owned(),
                ));
            }
            _ => {
                tokens.push(Token::Other);
                i += 1;
            }
        }
    }
    tokens
}

fn is_word(token: &Token, word: &str) -> bool {
    matches!(token, Token::Word(w) if w.eq_ignore_ascii_case(word))
}

/// `(folded column name, collation)` for every column of a CREATE TABLE
/// statement that declares a non-BINARY collation. Empty for anything that
/// is not a `CREATE TABLE [IF NOT EXISTS] name (...)`, optionally
/// TEMPORARY.
pub fn column_collations_from_create_table(sql: &str) -> Vec<(Box<str>, Box<str>)> {
    let tokens = tokenize(sql);
    let mut pos = 0;
    let mut next = || {
        let token = tokens.get(pos).cloned();
        pos += 1;
        token
    };
    if !next().is_some_and(|t| is_word(&t, "CREATE")) {
        return Vec::new();
    }
    let mut token = next();
    if token
        .as_ref()
        .is_some_and(|t| is_word(t, "TEMPORARY") || is_word(t, &"TEMPORARY"[..4]))
    {
        token = next();
    }
    if !token.is_some_and(|t| is_word(&t, "TABLE")) {
        return Vec::new();
    }
    // [IF NOT EXISTS] name [. name] (
    let mut open_at = None;
    for (idx, token) in tokens.iter().enumerate().skip(pos) {
        if *token == Token::Open {
            open_at = Some(idx);
            break;
        }
        if !matches!(token, Token::Word(_) | Token::Quoted(_) | Token::Other) {
            return Vec::new();
        }
        if is_word(token, "AS") {
            return Vec::new();
        }
    }
    let Some(open_at) = open_at else {
        return Vec::new();
    };
    let mut items: Vec<Vec<&Token>> = vec![Vec::new()];
    let mut depth = 0usize;
    for token in &tokens[open_at + 1..] {
        match token {
            Token::Open => depth += 1,
            Token::Close if depth == 0 => break,
            Token::Close => depth -= 1,
            Token::Comma if depth == 0 => {
                items.push(Vec::new());
                continue;
            }
            _ => {}
        }
        items.last_mut().expect("an item").push(token);
    }
    let mut out = Vec::new();
    for item in items {
        let Some(first) = item.first() else {
            continue;
        };
        let name = match first {
            Token::Word(word) => {
                if ["CONSTRAINT", "PRIMARY", "UNIQUE", "CHECK", "FOREIGN"]
                    .iter()
                    .any(|keyword| word.eq_ignore_ascii_case(keyword))
                {
                    continue;
                }
                word
            }
            Token::Quoted(name) => name,
            _ => continue,
        };
        let mut collation = None;
        let mut depth = 0usize;
        for (idx, token) in item.iter().enumerate().skip(1) {
            match token {
                Token::Open => depth += 1,
                Token::Close => depth = depth.saturating_sub(1),
                _ if depth == 0 && is_word(token, "COLLATE") => {
                    if let Some(Token::Word(value) | Token::Quoted(value)) = item.get(idx + 1) {
                        collation = canonical_collation(value);
                    }
                }
                _ => {}
            }
        }
        if let Some(collation) = collation {
            out.push((name.to_ascii_lowercase().into_boxed_str(), collation));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_names() {
        assert_eq!(canonical_collation("nocase").as_deref(), Some(NOCASE));
        assert_eq!(canonical_collation("\"RTRIM\"").as_deref(), Some(RTRIM));
        assert_eq!(canonical_collation("BINARY"), None);
        assert_eq!(canonical_collation("\"C\""), None);
        assert_eq!(canonical_collation("en-x-icu").as_deref(), Some("en-x-icu"));
        assert_eq!(&*explicit_key_collation("binary"), BINARY);
        assert!(same_collation(None, Some("BINARY")));
        assert!(!same_collation(None, Some("NOCASE")));
        assert!(same_collation(Some("NOCASE"), Some("nocase")));
        assert!(is_index_key_collation(Some(RTRIM)));
        assert!(!is_index_key_collation(Some("en-x-icu")));
        assert_eq!(normalize_key_text("AbC", Some(NOCASE)), "abc");
        assert_eq!(normalize_key_text("ab  ", Some(RTRIM)), "ab");
        assert_eq!(normalize_key_text("Ab ", None), "Ab ");
    }
}
