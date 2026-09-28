use std::collections::BTreeSet;
use std::path::Path;

use super::{KnownFailures, RunSummary};
use crate::sqlite_parity::runner::{CaseFailure, VerdictReason};
use crate::sqlite_parity::test_fixtures::unique_case;

fn baseline(entries: &str) -> KnownFailures {
    try_baseline(entries).expect("valid baseline")
}

fn try_baseline(entries: &str) -> anyhow::Result<KnownFailures> {
    let text = format!(
        r#"{{"schema_version":"redline-testing-sqlite-known-failures-v1","description":"test","failures":[{entries}]}}"#
    );
    KnownFailures::parse(text.as_bytes(), Path::new("known-failures.json"))
}

fn entry(suite: &str, case_id: &str, name: &str, stage: &str) -> String {
    format!(
        r#"{{"suite":"{suite}","case_id":"{case_id}","name":"{name}","stage":"{stage}","reason":"r","owner":"o"}}"#
    )
}

fn failure(case_id: &str, name: &str, reasons: &[VerdictReason]) -> CaseFailure {
    CaseFailure {
        case_id: case_id.to_owned(),
        name: name.to_owned(),
        verdict_reasons: reasons.iter().copied().collect(),
    }
}

/// A run of `passed` passing cases plus `failures`.
fn summary(passed: usize, failures: Vec<CaseFailure>) -> RunSummary {
    let mut summary = RunSummary::default();
    for index in 0..passed {
        summary.record_run(format!("{:05}", 90_000 + index), None);
    }
    for failure in failures {
        summary.record_run(failure.case_id.clone(), Some(failure));
    }
    summary
}

const TARGET: VerdictReason = VerdictReason::TargetSemanticFailure;
const DIFFERENTIAL: VerdictReason = VerdictReason::DifferentialMismatch;

#[test]
fn listed_failures_pass_the_gate_and_stay_failures() {
    let known = baseline(&entry(
        "sqlite_parity",
        "10547",
        "UNIQUE_CONSTRAINT_FAILED",
        "target_semantic_failure",
    ));
    let run = summary(
        9,
        vec![failure("10547", "UNIQUE_CONSTRAINT_FAILED", &[TARGET])],
    );
    known.gate("sqlite_parity", &run).expect("listed failure");
    assert_eq!((run.passed, run.failed, run.skipped), (9, 1, 0));
    assert_eq!(
        known.listed("sqlite_parity"),
        BTreeSet::from(["10547".to_owned()])
    );
    assert!(known.listed("memory").is_empty());
}

#[test]
fn an_unlisted_failure_is_fatal() {
    let error = KnownFailures::none()
        .gate(
            "sqlite_parity",
            &summary(
                1,
                vec![failure("10547", "UNIQUE_CONSTRAINT_FAILED", &[TARGET])],
            ),
        )
        .expect_err("unlisted failure");
    let text = format!("{error:#}");
    assert!(text.contains("10547 UNIQUE_CONSTRAINT_FAILED failed (target_semantic_failure) and is not in the baseline"), "{text}");
    // A baseline for one suite does not cover the other.
    let known = baseline(&entry(
        "sqlite_parity",
        "10547",
        "U",
        "target_semantic_failure",
    ));
    assert!(
        known
            .gate(
                "memory",
                &summary(0, vec![failure("10547", "U", &[TARGET])])
            )
            .is_err()
    );
    // rql_phase1 has no baseline at all.
    assert!(
        known
            .gate(
                "rql_phase1",
                &summary(0, vec![failure("10547", "U", &[TARGET])])
            )
            .is_err()
    );
}

#[test]
fn a_listed_case_that_passes_must_leave_the_baseline() {
    let known = baseline(&entry(
        "memory",
        "00076",
        "EXPLAIN_BYTECODE",
        "target_semantic_failure",
    ));
    let error = known
        .gate("memory", &summary(10, Vec::new()))
        .expect_err("stale entry");
    assert!(
        format!("{error:#}").contains(
            "00076 EXPLAIN_BYTECODE is listed as failing but passed: remove it from the baseline"
        ),
        "{error:#}"
    );
    // A listed case the run skipped is not confirmed either way.
    let mut run = summary(10, Vec::new());
    run.record_skip(&{
        let mut case = unique_case();
        case.id = 76;
        case
    });
    let error = known.gate("memory", &run).expect_err("skipped entry");
    assert!(format!("{error:#}").contains("was skipped"), "{error:#}");
}

#[test]
fn a_failure_of_another_kind_is_fatal() {
    let known = baseline(&entry(
        "sqlite_parity",
        "00134",
        "DOT_CRLF",
        "differential_mismatch",
    ));
    for reasons in [vec![TARGET], vec![TARGET, DIFFERENTIAL]] {
        let error = known
            .gate(
                "sqlite_parity",
                &summary(0, vec![failure("00134", "DOT_CRLF", &reasons)]),
            )
            .expect_err("reclassified failure");
        assert!(
            format!("{error:#}").contains("is listed as differential_mismatch but failed as"),
            "{error:#}"
        );
    }
    known
        .gate(
            "sqlite_parity",
            &summary(0, vec![failure("00134", "DOT_CRLF", &[DIFFERENTIAL])]),
        )
        .expect("same kind");
}

#[test]
fn malformed_baselines_are_rejected() {
    for (entries, needle) in [
        (
            entry("rql_phase1", "00001", "N", "target_semantic_failure"),
            "suite must be one of",
        ),
        (
            entry("sqlite_parity", "1", "N", "target_semantic_failure"),
            "five-digit",
        ),
        (
            entry("sqlite_parity", "00001", "N", "reference_contract_failure"),
            "not a target failure",
        ),
        (
            entry("sqlite_parity", "00001", "N", "passed"),
            "not a target failure",
        ),
        (
            // A timed-out or capped run fails differently from run to run.
            entry("sqlite_parity", "00001", "N", "execution_failure"),
            "not a target failure",
        ),
        (
            entry("sqlite_parity", "00001", " ", "target_semantic_failure"),
            "name is empty",
        ),
        (
            format!(
                "{},{}",
                entry("memory", "00001", "N", "target_semantic_failure"),
                entry("memory", "00001", "N", "differential_mismatch")
            ),
            "listed twice",
        ),
    ] {
        let error = try_baseline(&entries).expect_err(needle);
        assert!(format!("{error:#}").contains(needle), "{needle}: {error:#}");
    }
    let unknown_field = r#"{"schema_version":"redline-testing-sqlite-known-failures-v1","description":"d","failures":[],"extra":1}"#;
    assert!(KnownFailures::parse(unknown_field.as_bytes(), Path::new("k")).is_err());
    let old_schema = r#"{"schema_version":"v0","description":"d","failures":[]}"#;
    assert!(KnownFailures::parse(old_schema.as_bytes(), Path::new("k")).is_err());
}

#[test]
fn entries_must_name_real_corpus_cases() {
    let corpus = crate::sqlite_parity::all_cases().expect("corpus");
    baseline(&entry(
        "sqlite_parity",
        "10547",
        "UNIQUE_CONSTRAINT_FAILED",
        "target_semantic_failure",
    ))
    .check_corpus(&corpus)
    .expect("real case");
    let renamed = baseline(&entry(
        "sqlite_parity",
        "10547",
        "WRONG",
        "target_semantic_failure",
    ));
    assert!(renamed.check_corpus(&corpus).is_err());
    let missing = baseline(&entry("memory", "09999", "GONE", "target_semantic_failure"));
    assert!(missing.check_corpus(&corpus).is_err());
}

#[test]
fn baseline_identity_is_the_file_hash() {
    let known = baseline("");
    let source = known.source().expect("source");
    assert_eq!(source.path, Path::new("known-failures.json"));
    assert_eq!(source.sha256.len(), 64);
    assert!(KnownFailures::none().source().is_none());
}
