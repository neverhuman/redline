//! The raw-record fields the README, the CHANGELOG, evidence_processor and
//! the known-failures baseline read, as serialized: `status`,
//! `verdict_reason`, `stage` and `diagnostic` (SQ-02a), and the comparison
//! rules a record was judged by, `normalization_policy` and
//! `comparison_mode` (SQ-06). A renamed serde attribute, or a record that
//! copies the wrong verdict, changes the published schema; these tests fail
//! first.

use serde_json::{Value, json};

use super::case::ComparisonMode;
use super::order::{FirstEngine, MeasurementOrder};
use super::report::{
    RunLabels, SamplePosition, compare_record, selection_failure_record, skipped_compare_record,
};
use super::runner::{Verdict, VerdictReason, VerdictStage};
use super::test_fixtures::{output, unique_case};

fn labels() -> RunLabels<'static> {
    RunLabels {
        reference_engine: "sqlite3",
        target_engine: "redlinedb",
        sqlite_version: Some("3.53.1".to_owned()),
        measurement_order: MeasurementOrder::SqliteFirst,
    }
}

fn fields(record: &impl serde::Serialize) -> Value {
    let value = serde_json::to_value(record).expect("serialize record");
    json!({
        "status": value["status"],
        "verdict_reason": value["verdict_reason"],
        "stage": value["stage"],
        "diagnostic": value["diagnostic"],
        "sample_role": value["sample_role"],
        "normalization_policy": value["normalization_policy"],
        "comparison_mode": value["comparison_mode"],
    })
}

#[test]
fn a_judged_sample_records_its_verdict_and_comparison_rules() {
    let case = unique_case();
    let verdict = Verdict::failed(
        VerdictReason::ReferenceContractFailure,
        VerdictStage::ReferenceContract,
        "reference contract: sqlite3 exited 0; case expects exit 1".to_owned(),
    );
    let record = compare_record(
        &case,
        &output("sqlite3", Some(0), "", ""),
        &output("redlinedb", Some(0), "", ""),
        &labels(),
        SamplePosition {
            sample_index: 1,
            repetition_index: Some(1),
            sample_role: "measured:1".to_owned(),
            first_engine: FirstEngine::Reference,
        },
        &verdict,
        None,
    );
    assert_eq!(
        fields(&record),
        json!({
            "status": "failed",
            "verdict_reason": "reference_contract_failure",
            "stage": "reference_contract",
            "diagnostic": "reference contract: sqlite3 exited 0; case expects exit 1",
            "sample_role": "measured:1",
            "normalization_policy": "sqlite-parity-compare-v2",
            "comparison_mode": "cli_bytes_exact",
        })
    );

    for (verdict, reason, stage, status) in [
        (Verdict::passed(), "passed", "differential", "passed"),
        (
            Verdict::failed(
                VerdictReason::DifferentialMismatch,
                VerdictStage::Differential,
                "x".to_owned(),
            ),
            "differential_mismatch",
            "differential",
            "failed",
        ),
        (
            Verdict::failed(
                VerdictReason::TargetSemanticFailure,
                VerdictStage::TargetContract,
                "x".to_owned(),
            ),
            "target_semantic_failure",
            "target_contract",
            "failed",
        ),
        (
            Verdict::failed(
                VerdictReason::ExecutionFailure,
                VerdictStage::Execution,
                "x".to_owned(),
            ),
            "execution_failure",
            "execution",
            "failed",
        ),
    ] {
        let mut case = unique_case();
        case.comparison_mode = ComparisonMode::CliTextLf;
        let record = compare_record(
            &case,
            &output("sqlite3", Some(1), "", ""),
            &output("redlinedb", Some(1), "", ""),
            &labels(),
            SamplePosition {
                sample_index: 0,
                repetition_index: None,
                sample_role: "warmup".to_owned(),
                first_engine: FirstEngine::Target,
            },
            &verdict,
            None,
        );
        let value = fields(&record);
        assert_eq!(value["verdict_reason"], reason, "{value}");
        assert_eq!(value["stage"], stage, "{value}");
        assert_eq!(value["status"], status, "{value}");
        assert_eq!(value["comparison_mode"], "cli_text_lf", "{value}");
    }
}

#[test]
fn a_skipped_or_unselected_case_records_selection() {
    let case = unique_case();
    let record = skipped_compare_record(
        &case,
        &labels(),
        None,
        Some("skipped by policy".to_owned()),
        "SCOPE-1",
    );
    let value = serde_json::to_value(&record).expect("serialize");
    assert_eq!(
        fields(&record),
        json!({
            "status": "skipped",
            "verdict_reason": "skipped",
            "stage": "selection",
            "diagnostic": "skipped by policy",
            "sample_role": "skipped",
            "normalization_policy": "sqlite-parity-compare-v2",
            "comparison_mode": "cli_bytes_exact",
        })
    );
    assert_eq!(value["policy_exception_id"], "SCOPE-1");
    assert_eq!(value["execution_outcome"], "not_run");

    let verdict = Verdict::failed(
        VerdictReason::ReferenceCapabilityMissing,
        VerdictStage::Selection,
        "the reference lacks fts5".to_owned(),
    );
    let record = selection_failure_record(&case, &labels(), "artifact".into(), &verdict);
    let value = serde_json::to_value(&record).expect("serialize");
    assert_eq!(
        fields(&record),
        json!({
            "status": "failed",
            "verdict_reason": "reference_capability_missing",
            "stage": "selection",
            "diagnostic": "the reference lacks fts5",
            "sample_role": "not_run",
            "normalization_policy": "sqlite-parity-compare-v2",
            "comparison_mode": "cli_bytes_exact",
        })
    );
    assert_eq!(value["policy_exception_id"], Value::Null);
}
