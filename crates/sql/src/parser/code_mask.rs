//! Run a byte-wise rewrite pass over code only.
//!
//! Several compatibility passes scan the SQL text themselves and track
//! `'..'` and `".."` on their own. They did not know `[..]`, backticks,
//! comments, `E'..'` or `$tag$..$tag$`, so they rewrote bracket- and
//! backtick-quoted names, and an apostrophe inside a comment put their
//! quote tracking out of step so a later literal was rewritten as code
//! (Q5-01). [`rewrite_code_only`] hands such a pass a copy of the statement
//! in which every quoted name, comment and non-standard string is a
//! placeholder the pass cannot mistake for code, then puts the original
//! text back.
//!
//! Plain `'..'` literals stay as they are: the passes parse them as operands
//! (a GLOB pattern, a date modifier) and already skip their contents. A
//! quoted name becomes `"<marker n>"` and a Postgres `E'..'` or dollar
//! string `'<marker n>'`, so a pass still sees an identifier or a string in
//! that position; a comment keeps its `--` or `/* */` around the marker.

use super::code_scan::{Lexer, non_code_end};

/// Brackets a placeholder number. Private-use characters no SQL text in a
/// code position contains; a statement that does contain one is passed
/// through unmasked.
const OPEN: char = '\u{F8F0}';
const CLOSE: char = '\u{F8F1}';

/// What a masked span looked like to the pass, and what it was.
struct Span {
    /// The text written in place of the span: delimiters around the marker.
    masked: String,
    original: String,
}

/// `sql` with its non-code spans replaced by placeholders, and the spans.
struct Masked {
    text: String,
    spans: Vec<Span>,
}

/// Which `[..]` spans a pass itself parses and must still see. In the
/// SQLite dialect `[..]` quotes a name; RedlineDB also reads the Postgres
/// array constructor and subscripts there.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeepBrackets {
    /// Every `[..]` is a quoted name; mask it.
    No,
    /// Keep `ARRAY[..]` (the array and jsonb `?|` / `?&` passes).
    Array,
    /// Keep `ARRAY[..]` and a subscript written right after an operand
    /// (`x[1]`, `(a)[1]`), for the postfix subscript pass.
    Subscripts,
}

/// Whether the `[` at `open` belongs to the pass under `brackets`.
fn keeps_bracket(sql: &str, open: usize, brackets: KeepBrackets) -> bool {
    let before = sql[..open].trim_end();
    let after_array = before.len() >= 5
        && before.is_char_boundary(before.len() - 5)
        && before[before.len() - 5..].eq_ignore_ascii_case("array")
        && !before[..before.len() - 5]
            .bytes()
            .next_back()
            .is_some_and(super::code_scan::is_word_byte);
    match brackets {
        KeepBrackets::No => false,
        KeepBrackets::Array => after_array,
        KeepBrackets::Subscripts => {
            after_array
                || sql[..open]
                    .bytes()
                    .next_back()
                    .is_some_and(|b| super::code_scan::is_word_byte(b) || b == b')' || b == b']')
        }
    }
}

fn mask(sql: &str, lexer: Lexer, brackets: KeepBrackets) -> Masked {
    let bytes = sql.as_bytes();
    let mut text = String::with_capacity(sql.len());
    let mut spans = Vec::new();
    let mut copied = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let Some(end) = non_code_end(bytes, i, lexer) else {
            i += 1;
            continue;
        };
        let (open, close) = match bytes[i] {
            // A plain string literal: the passes parse these themselves.
            b'\'' => {
                i = end;
                continue;
            }
            b'[' if keeps_bracket(sql, i, brackets) => {
                i = end;
                continue;
            }
            b'"' | b'`' | b'[' => ("\"", "\""),
            b'-' => ("--", ""),
            b'/' => ("/*", "*/"),
            // `E'..'` and `$tag$..$tag$` read as an ordinary string.
            _ => ("'", "'"),
        };
        text.push_str(&sql[copied..i]);
        let masked = format!("{open}{OPEN}{}{CLOSE}{close}", spans.len());
        text.push_str(&masked);
        spans.push(Span {
            masked,
            original: sql[i..end].to_owned(),
        });
        copied = end;
        i = end;
    }
    text.push_str(&sql[copied..]);
    Masked { text, spans }
}

/// Put each span back: its masked form where the pass kept it whole, or the
/// bare marker where the pass moved or dropped the delimiters.
fn unmask(text: &str, spans: &[Span]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(OPEN) {
        let Some(len) = rest[at..].find(CLOSE) else {
            break;
        };
        let number = &rest[at + OPEN.len_utf8()..at + len];
        let Some(span) = number.parse::<usize>().ok().and_then(|n| spans.get(n)) else {
            break;
        };
        // Where does the masked form start? The pass may have kept the
        // opening delimiter right before the marker.
        let open_len = span.masked.find(OPEN).unwrap_or(0);
        let close_len =
            span.masked.len() - open_len - (number.len() + OPEN.len_utf8() + CLOSE.len_utf8());
        let marker_end = at + len + CLOSE.len_utf8();
        let whole_start = at.checked_sub(open_len);
        let whole = whole_start
            .and_then(|start| rest.get(start..marker_end + close_len))
            .filter(|candidate| *candidate == span.masked);
        match (whole, whole_start) {
            (Some(_), Some(start)) => {
                out.push_str(&rest[..start]);
                out.push_str(&span.original);
                rest = &rest[marker_end + close_len..];
            }
            _ => {
                out.push_str(&rest[..at]);
                out.push_str(&span.original);
                rest = &rest[marker_end..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Run `pass` over the code of `sql` only (see the module docs).
pub(crate) fn rewrite_code_only(
    sql: &str,
    brackets: KeepBrackets,
    pass: impl FnOnce(&str) -> String,
) -> String {
    if sql.contains(OPEN) || sql.contains(CLOSE) {
        return pass(sql);
    }
    let masked = mask(sql, Lexer::current(), brackets);
    if masked.spans.is_empty() {
        return pass(sql);
    }
    unmask(&pass(&masked.text), &masked.spans)
}

/// [`rewrite_code_only`] for a pass that answers `None` when it changed
/// nothing.
pub(crate) fn rewrite_code_only_opt(
    sql: &str,
    brackets: KeepBrackets,
    pass: impl FnOnce(&str) -> Option<String>,
) -> Option<String> {
    if sql.contains(OPEN) || sql.contains(CLOSE) {
        return pass(sql);
    }
    let masked = mask(sql, Lexer::current(), brackets);
    if masked.spans.is_empty() {
        return pass(sql);
    }
    pass(&masked.text).map(|out| unmask(&out, &masked.spans))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(sql: &str, lexer: Lexer, brackets: KeepBrackets) -> (String, String) {
        let masked = mask(sql, lexer, brackets);
        let back = unmask(&masked.text, &masked.spans);
        (masked.text, back)
    }

    #[test]
    fn masks_names_and_comments_and_restores_them() {
        let sql = "SELECT [a GLOB b], `c ? 'k'`, \"d\" /* it's */ 'x GLOB y' -- é\n";
        let (masked, back) = round_trip(sql, Lexer::Sqlite, KeepBrackets::No);
        assert_eq!(back, sql);
        assert!(!masked.contains("GLOB b"), "{masked}");
        assert!(!masked.contains("it's"), "{masked}");
        assert!(masked.contains("'x GLOB y'"), "{masked}");
        let sql = "SELECT a[1], ARRAY [2], 7 AS [b] -- x";
        let (masked, back) = round_trip(sql, Lexer::Sqlite, KeepBrackets::Subscripts);
        assert!(
            masked.starts_with("SELECT a[1], ARRAY [2], 7 AS \""),
            "{masked}"
        );
        assert_eq!(back, sql);
        let (masked, _) = round_trip(sql, Lexer::Sqlite, KeepBrackets::Array);
        assert!(masked.starts_with("SELECT a\""), "{masked}");
        assert!(masked.contains("ARRAY [2]"), "{masked}");
    }

    #[test]
    fn postgres_strings_read_as_plain_strings() {
        let sql = "SELECT $$array[1,2]$$, E'a\\'b', $q$x$q$, a[1]";
        let (masked, back) = round_trip(sql, Lexer::Postgres, KeepBrackets::Subscripts);
        assert_eq!(back, sql);
        assert!(!masked.contains("array[1,2]"), "{masked}");
        assert!(masked.ends_with("a[1]"), "{masked}");
    }

    #[test]
    fn a_pass_that_moves_a_span_keeps_its_text() {
        let masked = mask("x GLOB [p q]", Lexer::Sqlite, KeepBrackets::No);
        // A pass may reorder operands and drop delimiters.
        let moved = masked.text.replace("x GLOB ", "glob(") + ", x)";
        let bare = moved.replace('"', "");
        assert_eq!(unmask(&moved, &masked.spans), "glob([p q], x)");
        assert_eq!(unmask(&bare, &masked.spans), "glob([p q], x)");
    }
}
