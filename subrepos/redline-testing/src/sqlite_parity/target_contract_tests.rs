//! The target is held to the case's declared contract (SQ-02), and the
//! differential compares the bytes each shell printed (SQ-06).

use super::case::ComparisonMode;
use super::catalog::all_cases;
use super::runner::{Verdict, VerdictReason, VerdictStage, judge_sample};
use super::test_fixtures::{output, plain_case, unique_case};

fn corpus_case(id: usize) -> super::case::Case {
    all_cases()
        .expect("sqlite parity corpus")
        .into_iter()
        .find(|case| case.id == id)
        .unwrap_or_else(|| panic!("corpus case {id}"))
}

#[test]
fn case_10547_no_such_table_target_fails() {
    // Both shells exit 1, so exit codes agree and stdout is empty on both
    // sides. Only the declared fragment tells a UNIQUE violation from a
    // target that lost the table.
    let case = corpus_case(10547);
    assert_eq!(
        case.expected_stderr_contains,
        ["UNIQUE constraint failed: t.x"]
    );
    let verdict = judge_sample(
        &case,
        &output(
            "sqlite3",
            Some(1),
            "",
            "Runtime error near line 7: UNIQUE constraint failed: t.x (19)\n",
        ),
        &output(
            "redlinedb",
            Some(1),
            "",
            "Error: near line 7: no such table: t\n",
        ),
    );
    assert!(!verdict.is_pass(), "{verdict:?}");
    assert_eq!(verdict.reason, VerdictReason::TargetSemanticFailure);
    assert_eq!(verdict.stage, VerdictStage::TargetContract);
    assert_eq!(verdict.status(), "failed");
    let diagnostic = verdict.diagnostic.expect("diagnostic");
    assert!(
        diagnostic
            .contains("redlinedb stderr lacks declared fragment \"UNIQUE constraint failed: t.x\"")
            && diagnostic.contains("no such table: t"),
        "{diagnostic}"
    );
    // The same target with the declared error passes.
    let verdict = judge_sample(
        &case,
        &output(
            "sqlite3",
            Some(1),
            "",
            "Runtime error near line 7: UNIQUE constraint failed: t.x (19)\n",
        ),
        &output(
            "redlinedb",
            Some(1),
            "",
            "Error: near line 7: UNIQUE constraint failed: t.x\n",
        ),
    );
    assert_eq!(verdict, Verdict::passed());
}

#[test]
fn compare_stdout_false_still_enforces_stderr_fragments() {
    let mut case = unique_case();
    case.compare_stdout = false;
    let verdict = judge_sample(
        &case,
        &output(
            "sqlite3",
            Some(1),
            "",
            "Runtime error near line 3: UNIQUE constraint failed: t.x (19)\n",
        ),
        &output("redlinedb", Some(1), "", "Error: constraint violation\n"),
    );
    assert!(!verdict.is_pass(), "{verdict:?}");
    assert_eq!(verdict.reason, VerdictReason::TargetSemanticFailure);
    // So do declared stdout fragments, and the expected exit code.
    let mut case = plain_case();
    case.compare_stdout = false;
    case.expected_stdout_contains = vec!["ResultRow".to_owned()];
    let reference = output("sqlite3", Some(0), "0|Init|0|4\n3|ResultRow|1|1\n", "");
    for target in [
        output("redlinedb", Some(0), "Project [1]\n", ""),
        output("redlinedb", Some(1), "3|ResultRow|1|1\n", ""),
        output("redlinedb", None, "3|ResultRow|1|1\n", ""),
    ] {
        let verdict = judge_sample(&case, &reference, &target);
        assert_eq!(
            verdict.reason,
            VerdictReason::TargetSemanticFailure,
            "{verdict:?}"
        );
    }
}

#[test]
fn trailing_space_significant() {
    let verdict = judge_sample(
        &plain_case(),
        &output("sqlite3", Some(0), "a \n", ""),
        &output("redlinedb", Some(0), "a\n", ""),
    );
    assert!(!verdict.is_pass(), "{verdict:?}");
    assert_eq!(verdict.reason, VerdictReason::DifferentialMismatch);
}

#[test]
fn extra_empty_row_significant() {
    // `SELECT 1 UNION ALL SELECT NULL` with the default empty null marker:
    // the second row is an empty line, and a target that drops it differs.
    let verdict = judge_sample(
        &plain_case(),
        &output("sqlite3", Some(0), "1\n\n", ""),
        &output("redlinedb", Some(0), "1\n", ""),
    );
    assert!(!verdict.is_pass(), "{verdict:?}");
    assert_eq!(verdict.reason, VerdictReason::DifferentialMismatch);
}

#[test]
fn text_null_vs_null_marker() {
    // `.mode tabs` with the default empty null marker: a trailing NULL
    // column is a separator followed by nothing, and a target that drops
    // the column differs.
    let verdict = judge_sample(
        &plain_case(),
        &output("sqlite3", Some(0), "1\t\n", ""),
        &output("redlinedb", Some(0), "1\n", ""),
    );
    assert!(!verdict.is_pass(), "{verdict:?}");
    assert_eq!(verdict.reason, VerdictReason::DifferentialMismatch);
}

#[test]
fn invalid_utf8_distinguishable() {
    // The reference prints the blob byte 0xAB raw; a target that prints
    // U+FFFD (EF BF BD) in its place printed different bytes.
    let verdict = judge_sample(
        &plain_case(),
        &output("sqlite3", Some(0), b"blob|\x01\xab\n", ""),
        &output("redlinedb", Some(0), b"blob|\x01\xef\xbf\xbd\n", ""),
    );
    assert!(!verdict.is_pass(), "{verdict:?}");
    assert_eq!(verdict.reason, VerdictReason::DifferentialMismatch);
}

#[test]
fn crlf_in_field_significant() {
    // `SELECT 'a' || char(13,10) || 'b'`: the CR inside the field is data.
    let verdict = judge_sample(
        &plain_case(),
        &output("sqlite3", Some(0), "a\r\nb\n", ""),
        &output("redlinedb", Some(0), "a\nb\n", ""),
    );
    assert!(!verdict.is_pass(), "{verdict:?}");
    assert_eq!(verdict.reason, VerdictReason::DifferentialMismatch);
}

#[test]
fn text_null_marker_identical_bytes_are_not_typed_evidence() {
    // With `.nullvalue NULL`, a NULL and the text 'NULL' print the same
    // bytes. Byte-exact comparison cannot tell them apart; the corpus
    // proves types through typeof() and quote(), and typed comparison is
    // SQ-08. This pins the limit so nothing claims more.
    let verdict = judge_sample(
        &plain_case(),
        &output("sqlite3", Some(0), "NULL|NULL\n", ""),
        &output("redlinedb", Some(0), "NULL|NULL\n", ""),
    );
    assert_eq!(verdict, Verdict::passed());
}

#[test]
fn invalid_utf8_mismatch_diagnostic_shows_the_bytes() {
    let verdict = judge_sample(
        &plain_case(),
        &output("sqlite3", Some(0), b"blob|\x01\xab\n", ""),
        &output("redlinedb", Some(0), b"blob|\x01\xef\xbf\xbd\n", ""),
    );
    let diagnostic = verdict.diagnostic.expect("diagnostic");
    assert!(
        diagnostic.contains("stdout mismatch at byte 6")
            && diagnostic.contains("\\xab")
            && diagnostic.contains("\\xef\\xbf\\xbd"),
        "{diagnostic}"
    );
}

#[test]
fn text_lf_mode_reads_crlf_as_lf_only() {
    let mut case = plain_case();
    case.comparison_mode = ComparisonMode::CliTextLf;
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(0), "1\n2\n", ""),
        &output("redlinedb", Some(0), "1\r\n2\n", ""),
    );
    assert_eq!(verdict, Verdict::passed());
    // Trailing whitespace is still significant in cli_text_lf.
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(0), "1\n2\n", ""),
        &output("redlinedb", Some(0), "1\r\n2 \n", ""),
    );
    assert_eq!(verdict.reason, VerdictReason::DifferentialMismatch);
}

#[test]
fn case_208_ignores_only_its_declared_trace_lines() {
    let case = corpus_case(208);
    assert_eq!(case.ignore_line_prefixes, ["trace.xRandomness("]);
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(0), "trace.xRandomness(8,...)\n1\n", ""),
        &output("redlinedb", Some(0), "1\n", ""),
    );
    assert_eq!(verdict, Verdict::passed());
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(0), "trace.xRandomness(8,...)\n1\n", ""),
        &output("redlinedb", Some(0), "1 \n", ""),
    );
    assert_eq!(verdict.reason, VerdictReason::DifferentialMismatch);
}

#[test]
fn stderr_on_success_is_compared_byte_for_byte() {
    let verdict = judge_sample(
        &plain_case(),
        &output("sqlite3", Some(0), "1\n", "warning\n"),
        &output("redlinedb", Some(0), "1\n", "warning \n"),
    );
    assert_eq!(verdict.reason, VerdictReason::DifferentialMismatch);
    assert!(
        verdict
            .diagnostic
            .as_deref()
            .is_some_and(|diagnostic| diagnostic.starts_with("stderr mismatch")),
        "{verdict:?}"
    );
}
