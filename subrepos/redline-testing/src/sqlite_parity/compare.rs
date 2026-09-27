//! What the differential compares (SQ-06): the bytes each shell wrote.
//!
//! Nothing is decoded, trimmed or folded unless the case asks for it. A
//! trailing space, a missing empty row, a CR inside a field and a raw 0xAB
//! byte that the target printed as U+FFFD are all differences.

use super::case::{Case, ComparisonMode};
use super::text::sanitize_identifier;

/// The comparison rules every raw record was judged by, recorded in each
/// record as `normalization_policy`. Bump it whenever what `comparable`
/// removes or rewrites changes.
///
/// v2: stdout and stderr are compared byte for byte; `cli_text_lf` reads
/// CRLF as LF and nothing else; `ignore_line_prefixes` drops whole lines;
/// the case's own tmp directory reads as `{{CASE_TMP}}`; declared fragments
/// and `expected_stdout` are matched on UTF-8-lossy text with line endings
/// normalized. Records without the field predate it (v1): stdout and stderr
/// were decoded lossily, CR and CRLF folded to LF, trailing whitespace
/// trimmed, and case 208 dropped its randomness trace by id.
pub const NORMALIZATION_POLICY: &str = "sqlite-parity-compare-v2";

/// Longest rendering of one side of a mismatch in a diagnostic.
const DIAGNOSTIC_BYTES: usize = 2048;

/// The bytes the differential compares for one stream of one engine.
pub(super) fn comparable(case: &Case, engine: &str, bytes: &[u8]) -> Vec<u8> {
    let mut bytes = match case.comparison_mode {
        ComparisonMode::CliBytesExact => bytes.to_vec(),
        ComparisonMode::CliTextLf => replace_all(bytes, b"\r\n", b"\n"),
    };
    if !case.ignore_line_prefixes.is_empty() {
        bytes = drop_lines(&bytes, &case.ignore_line_prefixes);
    }
    // Each engine runs in its own tmp directory, named for the case, the
    // engine and this process. A path into it is the same path.
    let marker = format!(
        "/{}-{}-{}",
        case.display_id(),
        sanitize_identifier(engine),
        std::process::id()
    );
    replace_all(&bytes, marker.as_bytes(), b"/{{CASE_TMP}}")
}

/// Output as the declared fragments and `expected_stdout` see it: decoded
/// lossily (the corpus stores text) with CR and CRLF read as LF, like
/// `xtask ship-gate`. Only for declared diagnostics, never the differential.
pub(super) fn contract_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

/// A rendering of bytes for a diagnostic: a quoted string when they are
/// ASCII, escaped bytes otherwise, so a raw 0xAB and a U+FFFD never look
/// alike.
pub(super) fn describe(bytes: &[u8]) -> String {
    let (shown, rest) = bytes.split_at(bytes.len().min(DIAGNOSTIC_BYTES));
    let mut text = match std::str::from_utf8(shown) {
        Ok(text) if text.is_ascii() => format!("{text:?}"),
        _ => format!("b\"{}\"", shown.escape_ascii()),
    };
    if !rest.is_empty() {
        text.push_str(&format!(" ... ({} more bytes)", rest.len()));
    }
    text
}

/// The offset of the first byte where `left` and `right` differ.
pub(super) fn first_difference(left: &[u8], right: &[u8]) -> usize {
    left.iter()
        .zip(right)
        .position(|(left, right)| left != right)
        .unwrap_or_else(|| left.len().min(right.len()))
}

fn replace_all(haystack: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    if needle.is_empty() {
        return haystack.to_vec();
    }
    let mut out = Vec::with_capacity(haystack.len());
    let mut rest = haystack;
    while let Some(index) = rest
        .windows(needle.len())
        .position(|window| window == needle)
    {
        out.extend_from_slice(&rest[..index]);
        out.extend_from_slice(replacement);
        rest = &rest[index + needle.len()..];
    }
    out.extend_from_slice(rest);
    out
}

/// `bytes` without the lines that start with any of `prefixes`. A dropped
/// line takes its terminating LF with it; every other byte stays.
fn drop_lines(bytes: &[u8], prefixes: &[String]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if !prefixes
            .iter()
            .any(|prefix| !prefix.is_empty() && line.starts_with(prefix.as_bytes()))
        {
            out.extend_from_slice(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{comparable, contract_text, describe, drop_lines, first_difference};
    use crate::sqlite_parity::case::ComparisonMode;
    use crate::sqlite_parity::test_fixtures::plain_case;

    #[test]
    fn bytes_exact_keeps_every_byte() {
        let case = plain_case();
        let raw = b"a \r\n\xab\t\n\n";
        assert_eq!(comparable(&case, "redlinedb", raw), raw);
    }

    #[test]
    fn text_lf_reads_crlf_as_lf_and_nothing_else() {
        let mut case = plain_case();
        case.comparison_mode = ComparisonMode::CliTextLf;
        assert_eq!(
            comparable(&case, "redlinedb", b"1\r\n2 \n\r3\n\n"),
            b"1\n2 \n\r3\n\n"
        );
    }

    #[test]
    fn ignored_lines_leave_the_rest_exact() {
        let mut case = plain_case();
        case.ignore_line_prefixes = vec!["trace.xRandomness(".to_owned()];
        assert_eq!(
            comparable(
                &case,
                "sqlite3",
                b"trace.xRandomness(8)\n1 \ntrace.xRandomness(4)"
            ),
            b"1 \n"
        );
        assert_eq!(drop_lines(b"a\nb", &[String::new()]), b"a\nb");
    }

    #[test]
    fn case_tmp_paths_read_the_same() {
        let case = plain_case();
        let path = |engine: &str| {
            format!(
                "/dev/shm/redline-testing/{}-{engine}-{}/x.db\n",
                case.display_id(),
                std::process::id()
            )
        };
        assert_eq!(
            comparable(&case, "sqlite3", path("sqlite3").as_bytes()),
            comparable(&case, "redlinedb", path("redlinedb").as_bytes())
        );
    }

    #[test]
    fn diagnostics_show_invalid_bytes() {
        assert_eq!(describe(b"a\n"), "\"a\\n\"");
        assert_eq!(describe(b"\x01\xab\n"), "b\"\\x01\\xab\\n\"");
        assert_eq!(describe("\u{fffd}".as_bytes()), "b\"\\xef\\xbf\\xbd\"");
        assert!(describe(&[b'x'; 3000]).ends_with("(952 more bytes)"));
        assert_eq!(first_difference(b"abc", b"abd"), 2);
        assert_eq!(first_difference(b"ab", b"abc"), 2);
        assert_eq!(contract_text(b"a\r\nb\rc\xab"), "a\nb\nc\u{fffd}");
    }
}
