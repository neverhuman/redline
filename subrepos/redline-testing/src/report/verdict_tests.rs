//! One disjoint verdict per case, from complete and unique samples (SQ-04).
//!
//! These drive `generate()` end to end: a raw file whose samples are
//! duplicated, incomplete for one case, or whose cases differ from the
//! manifest must not render, and a case with any failed sample is failed and
//! nothing else.

use std::fs;

use serde_json::{Value, json};

use super::generate;
use super::test_fixtures::{self as fixture, pretty};
use super::types::ReportOptions;

fn record(case_id: &str, role: &str, repetition: Option<usize>, status: &str) -> String {
    let value = json!({
        "case_id": case_id,
        "name": format!("CASE_{case_id}"),
        "case_file": format!("CASE_{case_id}.rs"),
        "priority": "P0",
        "profile": "memory",
        "category": "SMOKE",
        "sample_role": role,
        "repetition_index": repetition,
        "status": status,
        "reference_executable_path": fixture::SQLITE_PATH,
        "target_executable_path": fixture::TARGET_PATH,
        "reference_executable_sha256": fixture::SQLITE_SHA256,
        "target_executable_sha256": fixture::TARGET_SHA256,
        "reference_version": fixture::SQLITE_VERSION,
        "target_version": fixture::TARGET_VERSION,
        "reference_elapsed_ns": 2_000_000u64,
        "target_elapsed_ns": 4_000_000u64,
    });
    format!("{value}\n")
}

fn measured(case_id: &str, repetitions: &[usize], status: &str) -> String {
    repetitions
        .iter()
        .map(|repetition| {
            record(
                case_id,
                &format!("measured:{repetition}"),
                Some(*repetition),
                status,
            )
        })
        .collect()
}

/// Renders `raw_text` and returns its summary.json. Without `evidence` the
/// report is a local diagnostic, whose manifest is the cases it holds.
fn render(
    raw_text: &str,
    evidence: Option<Value>,
    repetitions: usize,
    warmup: usize,
) -> anyhow::Result<Value> {
    let root = tempfile::Builder::new()
        .prefix("redline-testing-verdicts-")
        .tempdir()
        .expect("temp root");
    let path = |name: &str| root.path().join(name);
    fs::write(path("raw.jsonl"), raw_text).expect("raw");
    fs::write(
        path("README.md"),
        "# R\n\n<!-- sqlite-parity-report:begin -->\n<!-- sqlite-parity-report:end -->\n",
    )
    .expect("readme");
    let official_evidence = evidence.map(|value| {
        fs::write(path("official-evidence.processed.json"), pretty(&value)).expect("evidence");
        path("official-evidence.processed.json")
    });
    generate(ReportOptions {
        suite: "sqlite_parity".to_owned(),
        input: path("raw.jsonl"),
        historical_run: official_evidence.is_some(),
        local_diagnostics: official_evidence.is_none(),
        official_evidence,
        run_provenance: None,
        out_dir: path("out"),
        readme: path("README.md"),
        plot: None,
        performance_histogram_plot: None,
        median_test_performance_plot: None,
        readme_latency: true,
        jankurai_score: None,
        updated_date: "2026-09-28".to_owned(),
        expected_repetitions: Some(repetitions),
        expected_warmup: Some(warmup),
        check: false,
        // Official evidence: the compiled-in corpus; a local diagnostic:
        // the cases the records hold.
        case_manifest: None,
    })?;
    let summary = fs::read_to_string(path("out").join("summary.json")).expect("summary");
    Ok(serde_json::from_str(&summary).expect("summary json"))
}

fn error_text(result: anyhow::Result<Value>) -> String {
    match result {
        Ok(summary) => panic!("rendered inconsistent samples: {summary}"),
        Err(error) => format!("{error:#}"),
    }
}

#[test]
fn verdict_rejects_duplicate_samples() {
    // Four measured records for case 00001: the extra measured:1 repeats
    // a sample instead of covering a missing one.
    let raw = format!(
        "{}{}",
        measured("00001", &[1, 1, 2, 3], "passed"),
        measured("00002", &[1, 2, 3], "passed")
    );
    let error = error_text(render(&raw, None, 3, 0));
    assert!(
        error.contains("case 00001: duplicate sample measured:1"),
        "{error}"
    );
}

#[test]
fn verdict_rejects_per_case_incomplete_reps() {
    // Case 00001 has repetition 1 only; 00002 has 1..3, so the run as a
    // whole shows every repetition index.
    let raw = format!(
        "{}{}",
        measured("00001", &[1], "passed"),
        measured("00002", &[1, 2, 3], "passed")
    );
    let error = error_text(render(&raw, None, 3, 0));
    assert!(
        error.contains("case 00001: expected measured repetitions 1..=3, found [1]"),
        "{error}"
    );
    // A missing warmup is per case too.
    let raw = format!(
        "{}{}{}",
        measured("00001", &[1, 2, 3], "passed"),
        record("00002", "warmup", None, "passed"),
        measured("00002", &[1, 2, 3], "passed")
    );
    let error = error_text(render(&raw, None, 3, 1));
    assert!(
        error.contains("case 00001: expected 1 warmup samples but found 0"),
        "{error}"
    );
}

#[test]
fn warmup_fail_plus_measured_pass_is_one_failed_zero_passed() {
    let raw = format!(
        "{}{}{}{}",
        record("00001", "warmup", None, "failed"),
        measured("00001", &[1, 2, 3], "passed"),
        record("00002", "warmup", None, "passed"),
        measured("00002", &[1, 2, 3], "passed")
    );
    let summary = render(&raw, None, 3, 1).expect("report");
    assert_eq!(summary["total_cases"], 2, "{summary}");
    assert_eq!(summary["failed_cases"], 1, "{summary}");
    assert_eq!(summary["passed_cases"], 1, "{summary}");
    assert_eq!(summary["skipped_cases"], 0, "{summary}");
}

#[test]
fn missing_or_replaced_case_same_total_rejected() {
    // Every corpus case but one, plus one the corpus does not have: the
    // right total, the wrong cases.
    let manifest = crate::sqlite_parity::all_cases()
        .expect("corpus")
        .into_iter()
        .map(|case| case.display_id())
        .collect::<Vec<_>>();
    let complete = manifest
        .iter()
        .map(|case_id| fixture::raw_case(case_id))
        .collect::<String>();
    render(
        &complete,
        Some(fixture::historical_evidence(&complete)),
        3,
        0,
    )
    .expect("the whole corpus renders");
    let replaced = manifest
        .iter()
        .skip(1)
        .map(|case_id| fixture::raw_case(case_id))
        .chain([fixture::raw_case("99999")])
        .collect::<String>();
    let error = error_text(render(
        &replaced,
        Some(fixture::historical_evidence(&replaced)),
        3,
        0,
    ));
    assert!(
        error.contains(&format!("missing [\"{}\"]", manifest[0])),
        "{error}"
    );
    assert!(error.contains("not in the manifest [\"99999\"]"), "{error}");
}
