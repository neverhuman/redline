//! How the sqlite3 3.53.1 shell prints a value it shows as text.
//!
//! A BLOB prints as its bytes up to the first NUL, as `sqlite3_column_text`
//! hands them to the shell; bytes that are not UTF-8 pass through as they
//! are. The `-escape` mode then decides how a control character in a TEXT
//! or BLOB value prints: `^X` by default (`ascii`: 0x01 as `^A`, ESC as
//! `^[`), its Unicode control picture with `symbol` (0x01 as U+2401), and
//! as the raw byte with `off`. A tab, a newline, and a carriage return that
//! ends a line always pass through; DEL and bytes above 0x7F are left alone.

use std::borrow::Cow;

/// sqlite3's `-escape` mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Escape {
    #[default]
    Ascii,
    Symbol,
    Off,
}

impl Escape {
    pub(crate) fn parse(name: &str) -> Option<Self> {
        match name {
            "ascii" => Some(Self::Ascii),
            "symbol" => Some(Self::Symbol),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

/// `bytes` with each control character printed as `mode` prints it.
pub(crate) fn escape(bytes: &[u8], mode: Escape) -> Cow<'_, [u8]> {
    if mode == Escape::Off || !(0..bytes.len()).any(|index| escapes(bytes, index)) {
        return Cow::Borrowed(bytes);
    }
    let mut out = Vec::with_capacity(bytes.len() + 8);
    for (index, &byte) in bytes.iter().enumerate() {
        if !escapes(bytes, index) {
            out.push(byte);
        } else if mode == Escape::Symbol {
            // U+2400 + byte, in UTF-8.
            out.extend_from_slice(&[0xe2, 0x90, 0x80 + byte]);
        } else {
            out.push(b'^');
            out.push(byte + 0x40);
        }
    }
    Cow::Owned(out)
}

/// A BLOB's bytes as the shell reads them as text: up to the first NUL.
pub(crate) fn blob_bytes(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    &bytes[..end]
}

/// The bytes sqlite3 prints by default for a BLOB shown as text.
pub(crate) fn blob_text(bytes: &[u8]) -> Vec<u8> {
    escape(blob_bytes(bytes), Escape::Ascii).into_owned()
}

/// Whether sqlite3's csv and tabs modes put a value in double quotes: it is
/// empty, holds the column separator, or holds a byte of the shell's
/// `needCsvQuote` set (control characters, space, `"`, `'`, DEL and every
/// byte above 0x7F). The shell decides on the value before escaping it.
pub(crate) fn needs_csv_quote(bytes: &[u8], separator: &str) -> bool {
    let separator = separator.as_bytes();
    bytes.is_empty()
        || bytes
            .iter()
            .any(|&byte| byte <= b' ' || byte == b'"' || byte == b'\'' || byte >= 0x7f)
        || (!separator.is_empty() && bytes.windows(separator.len()).any(|w| w == separator))
}

/// Whether the shell escapes the byte at `index`: a control character that
/// is not a tab, a newline, or a carriage return right before a newline.
fn escapes(bytes: &[u8], index: usize) -> bool {
    match bytes[index] {
        b'\t' | b'\n' => false,
        b'\r' => bytes.get(index + 1) != Some(&b'\n'),
        byte => byte < 0x20,
    }
}

#[cfg(test)]
mod tests {
    use super::{Escape, blob_text, escape, needs_csv_quote};

    #[test]
    fn blob_text_matches_the_sqlite_shell() {
        // Bytes the pinned sqlite3 prints for `SELECT x'…'` in list mode.
        assert_eq!(blob_text(&[0x01, 0xab]), b"^A\xab");
        assert_eq!(blob_text(b"A\0BC"), b"A");
        assert_eq!(blob_text(&[0]), b"");
        assert_eq!(
            blob_text(b"\tA\nB\r\nC\rD\x1b\x1f\x7f\x80\xff"),
            b"\tA\nB\r\nC^MD^[^_\x7f\x80\xff"
        );
    }

    #[test]
    fn escape_modes_match_the_sqlite_shell() {
        // Bytes the pinned sqlite3 prints for this text under each -escape.
        let text = b"\tA\nB\r\nC\rD\x1b\x7f";
        assert_eq!(&*escape(text, Escape::Ascii), b"\tA\nB\r\nC^MD^[\x7f");
        assert_eq!(
            &*escape(text, Escape::Symbol),
            b"\tA\nB\r\nC\xe2\x90\x8dD\xe2\x90\x9b\x7f"
        );
        assert_eq!(&*escape(text, Escape::Off), text);
        assert_eq!(&*escape(b"\x01x", Escape::Symbol), "\u{2401}x".as_bytes());
    }

    #[test]
    fn csv_quoting_follows_the_shell_table() {
        for quoted in [
            &b""[..],
            b"a b",
            b"\x01",
            b"\x7f",
            b"\xab",
            b"a\"b",
            b"'",
            b"a,b",
        ] {
            assert!(needs_csv_quote(quoted, ","), "{quoted:?}");
        }
        // Tabs mode: a comma is ordinary, the tab separator is not.
        for plain in [&b"A"[..], b"a;b", b"1.5", b"a,b"] {
            assert!(!needs_csv_quote(plain, "\t"), "{plain:?}");
        }
        assert!(needs_csv_quote(b"a\tb", "\t"));
    }
}
