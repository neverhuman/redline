//! Each check of the declared contract (`validate_expected`) isolated: the
//! exit code, and the stdout, stderr and stdout+stderr fragments, each the
//! only thing wrong with an otherwise agreeing pair of runs. The assertions
//! pin the stage and the diagnostic, so a check that is deleted or aimed at
//! the wrong stream cannot hide behind another check failing the case.

use super::runner::{Verdict, VerdictReason, VerdictStage, judge_sample};
use super::test_fixtures::{output, plain_case, unique_case};

const UNIQUE_ERROR: &str = "Runtime error near line 3: UNIQUE constraint failed: t.x (19)\n";

fn assert_fails(verdict: Verdict, reason: VerdictReason, stage: VerdictStage, needle: &str) {
    assert_eq!(verdict.reason, reason, "{verdict:?}");
    assert_eq!(verdict.stage, stage, "{verdict:?}");
    let diagnostic = verdict.diagnostic.expect("a failure has a diagnostic");
    assert!(
        diagnostic.contains(needle),
        "diagnostic lacks {needle:?}: {diagnostic}"
    );
}

#[test]
fn reference_exit_code_alone_breaks_the_contract() {
    // Every declared fragment is present and both engines agree; only the
    // reference's exit code is not the declared one.
    let case = unique_case();
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(0), "", UNIQUE_ERROR),
        &output("redlinedb", Some(0), "", UNIQUE_ERROR),
    );
    assert_fails(
        verdict,
        VerdictReason::ReferenceContractFailure,
        VerdictStage::ReferenceContract,
        "reference contract: sqlite3 exited 0; case expects exit 1",
    );

    // The commonest shape: exit 0, no fragments, stdout not compared. Two
    // engines that both fail agree on everything the differential checks.
    let mut case = plain_case();
    case.compare_stdout = false;
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(1), "", "Error: boom\n"),
        &output("redlinedb", Some(1), "", "Error: boom\n"),
    );
    assert_fails(
        verdict,
        VerdictReason::ReferenceContractFailure,
        VerdictStage::ReferenceContract,
        "reference contract: sqlite3 exited 1; case expects exit 0",
    );
}

#[test]
fn target_exit_code_alone_breaks_the_contract() {
    let case = unique_case();
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(1), "", UNIQUE_ERROR),
        &output("redlinedb", Some(0), "", UNIQUE_ERROR),
    );
    assert_fails(
        verdict,
        VerdictReason::TargetSemanticFailure,
        VerdictStage::TargetContract,
        "target contract: redlinedb exited 0; case expects exit 1",
    );
}

#[test]
fn combined_fragment_is_checked_on_stdout_and_stderr_together() {
    let mut case = plain_case();
    case.compare_stdout = false;
    case.expected_combined_contains = vec!["before-exit".to_owned()];

    // On neither stream: the reference breaks the contract.
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(0), "out\n", "err\n"),
        &output("redlinedb", Some(0), "out\n", "err\n"),
    );
    assert_fails(
        verdict,
        VerdictReason::ReferenceContractFailure,
        VerdictStage::ReferenceContract,
        "sqlite3 stdout+stderr lacks declared fragment \"before-exit\"",
    );

    // On stderr alone, or on stdout alone: the combined check passes.
    for (stdout, stderr) in [("", "before-exit\n"), ("before-exit\n", "")] {
        let verdict = judge_sample(
            &case,
            &output("sqlite3", Some(0), stdout, stderr),
            &output("redlinedb", Some(0), stdout, stderr),
        );
        assert_eq!(verdict, Verdict::passed(), "{stdout:?} {stderr:?}");
    }

    // The reference keeps it and the target does not.
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(0), "", "before-exit\n"),
        &output("redlinedb", Some(0), "", "after-exit\n"),
    );
    assert_fails(
        verdict,
        VerdictReason::TargetSemanticFailure,
        VerdictStage::TargetContract,
        "redlinedb stdout+stderr lacks declared fragment \"before-exit\"",
    );
}

#[test]
fn stdout_and_stderr_fragments_are_checked_on_their_own_stream() {
    let mut case = plain_case();
    case.compare_stdout = false;
    case.expected_stdout_contains = vec!["R163".to_owned()];
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(0), "", "R163\n"),
        &output("redlinedb", Some(0), "", "R163\n"),
    );
    assert_fails(
        verdict,
        VerdictReason::ReferenceContractFailure,
        VerdictStage::ReferenceContract,
        "sqlite3 stdout lacks declared fragment \"R163\"",
    );

    let case = unique_case();
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(1), "UNIQUE constraint failed: t.x\n", ""),
        &output("redlinedb", Some(1), "UNIQUE constraint failed: t.x\n", ""),
    );
    assert_fails(
        verdict,
        VerdictReason::ReferenceContractFailure,
        VerdictStage::ReferenceContract,
        "sqlite3 stderr lacks declared fragment \"UNIQUE constraint failed: t.x\"",
    );
}

#[test]
fn declared_stdout_mismatch_names_the_expected_stdout() {
    let mut case = plain_case();
    case.expected_stdout = Some("a   b\n".to_owned());
    let verdict = judge_sample(
        &case,
        &output("sqlite3", Some(0), "a    b\n", ""),
        &output("redlinedb", Some(0), "a    b\n", ""),
    );
    assert_fails(
        verdict,
        VerdictReason::ReferenceContractFailure,
        VerdictStage::ReferenceContract,
        "sqlite3 stdout differs from expected_stdout",
    );
}
