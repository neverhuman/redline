//! Unit tests for `compare`.

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
