//! Unit tests for `runner`.

use super::{Verdict, VerdictReason, VerdictStage, judge_sample, validate_compare};
use crate::sqlite_parity::bounded::ExecutionOutcome;
use crate::sqlite_parity::test_fixtures::{incomplete, output, plain_case, unique_case};

#[test]
fn incomplete_runs_fail_before_any_contract() {
    // A run killed at the deadline or the output cap, or one that never
    // started, left no whole result: neither the contract nor the
    // differential may judge it, on either side.
    let case = plain_case();
    let healthy = |engine| output(engine, Some(0), "", "");
    for (outcome, failure) in [
        (ExecutionOutcome::Timeout, "timed out after 500 ms"),
        (
            ExecutionOutcome::DeadlineExceeded,
            "deadline exceeded after 500 ms; leader exited 0; run rejected",
        ),
        (ExecutionOutcome::OutputLimit, "wrote more than 64 bytes"),
        (ExecutionOutcome::SpawnError, "could not run: spawn failed"),
    ] {
        for (reference, target, engine) in [
            (
                healthy("sqlite3"),
                incomplete("redlinedb", outcome, failure),
                "redlinedb",
            ),
            (
                incomplete("sqlite3", outcome, failure),
                healthy("redlinedb"),
                "sqlite3",
            ),
        ] {
            let verdict = judge_sample(&case, &reference, &target);
            assert_eq!(
                verdict.reason,
                VerdictReason::ExecutionFailure,
                "{verdict:?}"
            );
            assert_eq!(verdict.stage, VerdictStage::Execution);
            assert_eq!(verdict.status(), "failed");
            assert_eq!(
                verdict.diagnostic.as_deref(),
                Some(format!("{engine} {failure}").as_str())
            );
        }
    }
}

#[test]
fn deadline_with_matching_output_and_zero_exit_is_rejected_on_either_engine() {
    let case = plain_case();
    for engine in ["redlinedb", "sqlite3"] {
        let mut late = output(engine, Some(0), "2440952.5\n", "");
        late.outcome = ExecutionOutcome::DeadlineExceeded;
        late.failure = Some("deadline exceeded; normal leader exit; run rejected".to_owned());
        let other_engine = if engine == "redlinedb" {
            "sqlite3"
        } else {
            "redlinedb"
        };
        let healthy = output(other_engine, Some(0), "2440952.5\n", "");
        let (reference, target) = if engine == "redlinedb" {
            (healthy, late)
        } else {
            (late, healthy)
        };
        let verdict = judge_sample(&case, &reference, &target);
        assert_eq!(verdict.reason, VerdictReason::ExecutionFailure);
        assert_eq!(verdict.stage, VerdictStage::Execution);
        assert_eq!(verdict.status(), "failed");
    }
}

#[test]
fn reference_contract_violation_fails_even_when_engines_agree() {
    // Both engines exit 1 with the same wrong error: the reference does
    // not print the UNIQUE failure the case declares, so the agreement
    // proves nothing.
    let case = unique_case();
    let wrong_error = "Parse error near line 3: no such table: t\n";
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(1), "", wrong_error),
        &output("redlinedb", Some(1), "", wrong_error),
    );
    assert_eq!(verdict.reason, VerdictReason::ReferenceContractFailure);
    assert_eq!(verdict.stage, VerdictStage::ReferenceContract);
    assert_eq!(verdict.status(), "failed");
    let diagnostic = verdict.diagnostic.expect("failure diagnostic");
    assert!(
        diagnostic.contains("UNIQUE constraint failed: t.x"),
        "{diagnostic}"
    );

    // Both engines succeed where the case declares a failure. The exit
    // code is checked first, so it is what the diagnostic names (see
    // reference_contract_tests for each check on its own).
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(0), "1\n", ""),
        &output("redlinedb", Some(0), "1\n", ""),
    );
    assert_eq!(verdict.reason, VerdictReason::ReferenceContractFailure);
    assert!(
        verdict
            .diagnostic
            .as_deref()
            .is_some_and(|diagnostic| diagnostic.contains("exited 0; case expects exit 1")),
        "{verdict:?}"
    );

    // Both engines print the declared stdout fragment only on stderr.
    let mut stdout_case = unique_case();
    stdout_case.expected_exit = 0;
    stdout_case.expected_stderr_contains.clear();
    stdout_case.expected_stdout_contains = vec!["R163".to_owned()];
    let verdict = judge_sample(
        &stdout_case,
        &output("sqlite3", Some(0), "", "R163\n"),
        &output("redlinedb", Some(0), "", "R163\n"),
    );
    assert_eq!(verdict.reason, VerdictReason::ReferenceContractFailure);

    // Both engines agree on stdout that is not the declared stdout.
    let mut exact_case = stdout_case.clone();
    exact_case.expected_stdout_contains.clear();
    exact_case.expected_stdout = Some("a   b\n".to_owned());
    let verdict = judge_sample(
        &exact_case,
        &output("sqlite3", Some(0), "a    b\n", ""),
        &output("redlinedb", Some(0), "a    b\n", ""),
    );
    assert_eq!(verdict.reason, VerdictReason::ReferenceContractFailure);

    // The declared contract holds on the reference and the target agrees.
    let right_error = "Runtime error near line 3: UNIQUE constraint failed: t.x (19)\n";
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(1), "", right_error),
        &output("redlinedb", Some(1), "", right_error),
    );
    assert_eq!(verdict, Verdict::passed());
}

#[test]
fn reference_contract_checks_exact_stdout_after_normalization() {
    let mut case = unique_case();
    case.expected_exit = 0;
    case.expected_stderr_contains.clear();
    case.expected_stdout = Some("1\n2\n".to_owned());
    // The reference keeps the declared stdout once normalized ...
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(0), "1\r\n2\r\n  \n", ""),
        &output("redlinedb", Some(0), "1\r\n2\r\n  \n", ""),
    );
    assert_eq!(verdict, Verdict::passed());
    // ... but that normalization is the declared contract's alone: the
    // differential still compares the bytes each shell printed.
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(0), "1\r\n2\r\n  \n", ""),
        &output("redlinedb", Some(0), "1\n2\n", ""),
    );
    assert_eq!(verdict.reason, VerdictReason::DifferentialMismatch);
    // compare_stdout=false leaves stdout to the fragments.
    case.compare_stdout = false;
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(0), "other\n", ""),
        &output("redlinedb", Some(0), "different\n", ""),
    );
    assert_eq!(verdict, Verdict::passed());
}

#[test]
fn signal_terminated_children_never_pass() {
    let mut case = unique_case();
    case.expected_exit = 0;
    case.expected_stderr_contains.clear();
    let verdict = judge_sample(
        &case,
        &output("sqlite3", None, "", ""),
        &output("redlinedb", None, "", ""),
    );
    assert_eq!(verdict.reason, VerdictReason::ReferenceContractFailure);
    assert!(
        verdict
            .diagnostic
            .as_deref()
            .is_some_and(|diagnostic| diagnostic.contains("terminated by a signal")),
        "{verdict:?}"
    );
    // A healthy reference does not rescue a target killed by a signal:
    // the target breaks the case's contract before any comparison.
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(0), "", ""),
        &output("redlinedb", None, "", ""),
    );
    assert_eq!(verdict.reason, VerdictReason::TargetSemanticFailure);
    assert_eq!(verdict.stage, VerdictStage::TargetContract);
    assert!(
        verdict
            .diagnostic
            .as_deref()
            .is_some_and(|diagnostic| diagnostic.contains("redlinedb was terminated by a signal")),
        "{verdict:?}"
    );
    // Nor can the differential alone pass two signal deaths.
    assert!(
        validate_compare(
            &case,
            &output("sqlite3", None, "", ""),
            &output("redlinedb", None, "", "")
        )
        .is_err()
    );
}
