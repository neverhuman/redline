//! Where the SQL code is: a lexer that tells code apart from string
//! literals, quoted identifiers and comments.
//!
//! The compatibility rewrites in `parser::rewrite`, the CTE hint strip in
//! `parser::prepare` and the `pg_listening_channels()` rewrite change SQL
//! text before sqlparser sees it. They may only change code. The same words
//! inside `'...'`, `"..."` or a comment are user data or names, and a
//! rewrite there changes the answer (Q5-01, PG-02).

use std::ops::Range;

use crate::value::postgres_result_dialect;

/// Which lexical forms quote or comment out text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lexer {
    /// `'..'`, `".."`, `` `..` `` (doubled-quote escapes) and `[..]`;
    /// `--` line comments and non-nesting `/* */` block comments.
    Sqlite,
    /// `'..'` (`''` escape), `E'..'` (backslash and `''` escapes), `".."`,
    /// `$tag$..$tag$`, `--` line comments and nesting `/* */` comments.
    /// `[` is an array subscript here, not a quote.
    Postgres,
}

impl Lexer {
    /// The lexer of the dialect the current statement is prepared under.
    pub(crate) fn current() -> Self {
        if postgres_result_dialect() {
            Self::Postgres
        } else {
            Self::Sqlite
        }
    }
}

/// When `bytes[i]` starts a string literal, quoted identifier or comment,
/// the index just past its end. An unterminated one runs to the end of the
/// input. `None` when `bytes[i]` is code.
pub(crate) fn non_code_end(bytes: &[u8], i: usize, lexer: Lexer) -> Option<usize> {
    let postgres = lexer == Lexer::Postgres;
    match *bytes.get(i)? {
        quote @ (b'\'' | b'"') => Some(doubled_quote_end(bytes, i, quote)),
        b'`' if !postgres => Some(doubled_quote_end(bytes, i, b'`')),
        b'[' if !postgres => Some(bracket_end(bytes, i)),
        b'-' if bytes.get(i + 1) == Some(&b'-') => Some(line_comment_end(bytes, i)),
        b'/' if bytes.get(i + 1) == Some(&b'*') => Some(block_comment_end(bytes, i, postgres)),
        b'E' | b'e' if postgres && bytes.get(i + 1) == Some(&b'\'') && !follows_word(bytes, i) => {
            Some(escape_string_end(bytes, i + 1))
        }
        b'$' if postgres && !follows_word(bytes, i) => dollar_quote_end(bytes, i),
        _ => None,
    }
}

/// When `bytes[i]` starts a comment, the index just past it.
pub(crate) fn comment_end(bytes: &[u8], i: usize, lexer: Lexer) -> Option<usize> {
    match (bytes.get(i), bytes.get(i + 1)) {
        (Some(b'-'), Some(b'-')) | (Some(b'/'), Some(b'*')) => non_code_end(bytes, i, lexer),
        _ => None,
    }
}

/// Skip whitespace and comments from `i`.
pub(crate) fn skip_trivia(bytes: &[u8], mut i: usize, lexer: Lexer) -> usize {
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        match comment_end(bytes, i, lexer) {
            Some(end) => i = end,
            None => return i,
        }
    }
}

/// True for a byte that continues an identifier or keyword. Non-ASCII bytes
/// count, since both dialects accept them in unquoted names.
pub(crate) fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

fn follows_word(bytes: &[u8], i: usize) -> bool {
    i > 0 && is_word_byte(bytes[i - 1])
}

fn doubled_quote_end(bytes: &[u8], open: usize, quote: u8) -> usize {
    let mut i = open + 1;
    while i < bytes.len() {
        if bytes[i] == quote {
            if bytes.get(i + 1) == Some(&quote) {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    bytes.len()
}

fn bracket_end(bytes: &[u8], open: usize) -> usize {
    bytes[open + 1..]
        .iter()
        .position(|&b| b == b']')
        .map_or(bytes.len(), |rel| open + 1 + rel + 1)
}

fn line_comment_end(bytes: &[u8], start: usize) -> usize {
    bytes[start..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(bytes.len(), |rel| start + rel)
}

fn block_comment_end(bytes: &[u8], start: usize, nested: bool) -> usize {
    let mut depth = 1usize;
    let mut i = start + 2;
    while i < bytes.len() {
        if nested && bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            depth += 1;
            i += 2;
        } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            i += 1;
        }
    }
    bytes.len()
}

/// `E'..'`: a backslash escapes the next byte and `''` is a quote.
fn escape_string_end(bytes: &[u8], open: usize) -> usize {
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'\'' if bytes.get(i + 1) == Some(&b'\'') => i += 2,
            b'\'' => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// `$tag$..$tag$` or `$$..$$`. `$1` is a parameter and `$name` without a
/// closing `$` is not a quote; both return `None`.
fn dollar_quote_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut tag_end = start + 1;
    if bytes
        .get(tag_end)
        .is_some_and(|&b| b.is_ascii_alphabetic() || b == b'_' || b >= 0x80)
    {
        while tag_end < bytes.len()
            && (bytes[tag_end].is_ascii_alphanumeric()
                || bytes[tag_end] == b'_'
                || bytes[tag_end] >= 0x80)
        {
            tag_end += 1;
        }
    }
    if bytes.get(tag_end) != Some(&b'$') {
        return None;
    }
    let tag = &bytes[start..=tag_end];
    let body = tag_end + 1;
    Some(
        bytes[body..]
            .windows(tag.len())
            .position(|window| window == tag)
            .map_or(bytes.len(), |rel| body + rel + tag.len()),
    )
}

/// The byte ranges of `sql` that are code, in order. Range bounds are
/// always char boundaries: every literal and comment starts and ends at an
/// ASCII byte or at the end of the input.
pub(crate) fn code_ranges(sql: &str, lexer: Lexer) -> CodeRanges<'_> {
    CodeRanges {
        bytes: sql.as_bytes(),
        pos: 0,
        lexer,
    }
}

pub(crate) struct CodeRanges<'a> {
    bytes: &'a [u8],
    pos: usize,
    lexer: Lexer,
}

impl Iterator for CodeRanges<'_> {
    type Item = Range<usize>;

    fn next(&mut self) -> Option<Range<usize>> {
        while self.pos < self.bytes.len() {
            let start = self.pos;
            let mut i = start;
            while i < self.bytes.len() {
                if let Some(end) = non_code_end(self.bytes, i, self.lexer) {
                    self.pos = end;
                    break;
                }
                i += 1;
            }
            if i == self.bytes.len() {
                self.pos = i;
            }
            if i > start {
                return Some(start..i);
            }
        }
        None
    }
}

/// Copy the character that starts at byte `*i` of `sql` to `out` and step
/// past it. Byte-wise rewrites use this instead of `out.push(byte as char)`,
/// which turned each byte of a multi-byte UTF-8 character into a separate
/// Latin-1 character (`'é'` became `'Ã©'`). Callers advance only with this
/// helper or jump to ASCII delimiters, so `*i` is always a char boundary;
/// it never panics if that is broken.
pub(crate) fn copy_char(out: &mut String, sql: &str, i: &mut usize) {
    let mut end = *i + 1;
    while end < sql.len() && !sql.is_char_boundary(end) {
        end += 1;
    }
    if let Some(text) = sql.get(*i..end) {
        out.push_str(text);
    }
    *i = end;
}

/// An ASCII-lowercased copy of `sql` in which every byte of a literal,
/// quoted identifier or comment is a space. Byte offsets match `sql`, so a
/// pass that searched `sql.to_ascii_lowercase()` for its keywords can search
/// this instead, match code only, and still slice the original text.
pub(crate) fn code_lowercase(sql: &str) -> String {
    let mut out = vec![b' '; sql.len()];
    for range in code_ranges(sql, Lexer::current()) {
        out[range.clone()].copy_from_slice(&sql.as_bytes()[range]);
    }
    out.make_ascii_lowercase();
    // Code ranges start and end on char boundaries and the rest is ASCII.
    String::from_utf8(out).expect("code ranges are char-aligned")
}

/// Case-insensitive search for `needle_lower` in the code of `sql` only,
/// from byte `from`. Literals, quoted identifiers and comments never match.
pub(crate) fn find_code_ci(sql: &str, needle_lower: &[u8], from: usize) -> Option<usize> {
    // Most statements do not contain the needle at all; only lex when a
    // raw match exists.
    super::find_ignore_ascii_case(sql.get(from..)?, needle_lower)?;
    code_ranges(sql, Lexer::current()).find_map(|range| {
        let start = range.start.max(from);
        if start >= range.end {
            return None;
        }
        super::find_ignore_ascii_case(&sql[start..range.end], needle_lower).map(|rel| start + rel)
    })
}

/// True when `needle_lower` occurs, ignoring ASCII case, in the code of
/// `sql`. The gate every raw pre-parse rewrite checks before it runs.
pub(crate) fn sql_code_contains_ci(sql: &str, needle_lower: &[u8]) -> bool {
    find_code_ci(sql, needle_lower, 0).is_some()
}

/// Replace every exact occurrence of `needle` that lies in code.
pub(crate) fn replace_code(sql: &str, needle: &str, replacement: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut copied = 0usize;
    for range in code_ranges(sql, Lexer::current()) {
        let code = &sql[range.clone()];
        for (rel, _) in code.match_indices(needle) {
            let at = range.start + rel;
            out.push_str(&sql[copied..at]);
            out.push_str(replacement);
            copied = at + needle.len();
        }
    }
    out.push_str(&sql[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(sql: &str, lexer: Lexer) -> String {
        code_ranges(sql, lexer).map(|r| &sql[r]).collect()
    }

    #[test]
    fn sqlite_quotes_and_comments_are_not_code() {
        let sql = "SELECT 'a''b', \"c\"\"d\", `e`, [f g] -- h\n/* i */ x";
        assert_eq!(code(sql, Lexer::Sqlite), "SELECT , , ,  \n x");
    }

    #[test]
    fn sqlite_block_comments_do_not_nest() {
        assert_eq!(code("a /* /* */ b */ c", Lexer::Sqlite), "a  b */ c");
    }

    #[test]
    fn postgres_forms() {
        assert_eq!(
            code(
                "SELECT E'x\\'y', $$ a $$, $t$ b $t$, a[1], $1",
                Lexer::Postgres
            ),
            "SELECT , , , a[1], $1"
        );
        assert_eq!(code("a /* /* */ b */ c", Lexer::Postgres), "a  c");
        // `E` inside a word, and `$` inside a name, start nothing.
        assert_eq!(code("sizE'a\\' b", Lexer::Postgres), "sizE b");
        assert_eq!(code("a$b$ c", Lexer::Postgres), "a$b$ c");
    }

    #[test]
    fn unterminated_forms_run_to_the_end() {
        assert_eq!(code("a 'b", Lexer::Sqlite), "a ");
        assert_eq!(code("a /* b", Lexer::Sqlite), "a ");
        assert_eq!(code("a $$ b", Lexer::Postgres), "a ");
    }

    #[test]
    fn copy_char_keeps_multibyte_characters() {
        let sql = "aé€😀";
        let mut out = String::new();
        let mut i = 0;
        while i < sql.len() {
            copy_char(&mut out, sql, &mut i);
        }
        assert_eq!(out, sql);
    }

    #[test]
    fn search_and_replace_skip_literals() {
        let sql = "SELECT 'NULL IS NOT 1', NULL IS NOT 1 -- NULL IS NOT 1";
        assert_eq!(find_code_ci(sql, b"null is not 1", 0), Some(24));
        assert_eq!(
            code_lowercase("SELECT 'A' /* B */ Ü, \"C\""),
            format!("select{}Ü,{}", " ".repeat(13), " ".repeat(4))
        );
        assert!(!sql_code_contains_ci("SELECT 'into x'", b" into "));
        assert!(sql_code_contains_ci("SELECT 1 into x", b" into "));
        assert_eq!(
            replace_code(sql, "NULL IS NOT 1", "NULL IS DISTINCT FROM 1"),
            "SELECT 'NULL IS NOT 1', NULL IS DISTINCT FROM 1 -- NULL IS NOT 1"
        );
    }
}
