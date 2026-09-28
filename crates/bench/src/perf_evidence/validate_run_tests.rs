//! `validate-run` accepts exactly the requested run (BM3-01).

use std::fs;
use std::io::Cursor;
use std::path::Path;

use super::*;

const PLAN_CASES: [&str; 3] = ["10000", "10001", "10002"];

fn plan() -> RunPlan {
    RunPlan {
        expected_cases: 3,
        repetitions: 3,
        warmup: 1,
        case_manifest: Some(PLAN_CASES.iter().map(|id| (*id).to_owned()).collect()),
    }
}

fn row(
    case: &str,
    role: &str,
    sample_index: usize,
    repetition: Option<usize>,
    status: &str,
) -> String {
    serde_json::json!({
        "case_id": case,
        "status": status,
        "sample_role": role,
        "sample_index": sample_index,
        "repetition_index": repetition,
        "reference_elapsed_ns": 1_000,
        "target_elapsed_ns": 900,
        "latency_ratio": 0.9,
    })
    .to_string()
}

/// One case's records: one warmup, then measured:1..=3.
fn executed(case: &str, status: &str) -> Vec<String> {
    let mut rows = vec![row(case, "warmup", 0, None, status)];
    rows.extend((1..=3).map(|k| row(case, &format!("measured:{k}"), k, Some(k), status)));
    rows
}

fn complete_rows() -> Vec<String> {
    PLAN_CASES
        .iter()
        .flat_map(|case| executed(case, "passed"))
        .collect()
}

fn validate(lines: &[String], plan: &RunPlan) -> Result<RunValidation> {
    let text = lines
        .iter()
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    validate_rows(&parse_rows(Cursor::new(text))?, plan)
}

fn error_of(lines: &[String], plan: &RunPlan) -> String {
    format!(
        "{:#}",
        validate(lines, plan).expect_err("the run must be rejected")
    )
}

/// Writes `lines` as a raw file with a marker the runner would write.
fn write_run(dir: &Path, lines: &[String]) -> std::path::PathBuf {
    let raw = dir.join("sqlite_parity.jsonl");
    let text = lines
        .iter()
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    fs::write(&raw, &text).unwrap();
    let cases = lines
        .iter()
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["case_id"].to_string())
        .collect::<std::collections::BTreeSet<_>>();
    let marker = serde_json::json!({
        "schema_version": COMPLETION_SCHEMA,
        "suite": "sqlite_parity",
        "raw_file": "sqlite_parity.jsonl",
        "records": lines.len(),
        "cases": cases.len(),
        "raw_sha256": format!("{:x}", Sha256::digest(text.as_bytes())),
    });
    fs::write(completion_marker_path(&raw), marker.to_string()).unwrap();
    raw
}

#[test]
fn complete_run_validates() {
    let validation = validate(&complete_rows(), &plan()).expect("complete run");
    assert_eq!(
        (
            validation.cases,
            validation.executed_cases,
            validation.records,
            validation.failed_cases
        ),
        (3, 3, 12, 0)
    );
    // Failures are counted, not rejected: that is the known-failures
    // filter's job.
    let mut rows = complete_rows();
    rows.truncate(8);
    rows.extend(executed("10002", "failed"));
    assert_eq!(validate(&rows, &plan()).expect("complete").failed_cases, 1);
    // Placeholders stand for cases that did not run.
    let mut rows = complete_rows();
    rows.truncate(8);
    rows.push(
        serde_json::json!({"case_id": "10002", "status": "skipped", "sample_role": "skipped"})
            .to_string(),
    );
    let validation = validate(&rows, &plan()).expect("skip placeholder");
    assert_eq!(
        (validation.executed_cases, validation.skipped_cases),
        (2, 1)
    );
}

#[test]
fn missing_repetition_fails() {
    let mut rows = complete_rows();
    rows.remove(5); // 10001 measured:1
    let message = error_of(&rows, &plan());
    assert!(
        message.contains("case 10001: expected measured repetitions 1..=3, found [2, 3]"),
        "{message}"
    );
}

#[test]
fn duplicate_sample_fails() {
    let mut rows = complete_rows();
    rows.insert(2, rows[1].clone());
    let message = error_of(&rows, &plan());
    assert!(
        message.contains("line 3: case 10000: duplicate sample measured:1 (first on line 2)"),
        "{message}"
    );
}

#[test]
fn warmup_count_must_match_the_plan() {
    let mut rows = complete_rows();
    rows.remove(0);
    assert!(error_of(&rows, &plan()).contains("case 10000: expected 1 warmup samples, found 0"));
    let no_warmup = RunPlan {
        warmup: 0,
        ..plan()
    };
    assert!(
        error_of(&complete_rows(), &no_warmup).contains("a warmup sample in a run without warmups")
    );
    let extra_repetition = row("10000", "measured:4", 4, Some(4), "passed");
    let mut rows = complete_rows();
    rows.push(extra_repetition);
    assert!(error_of(&rows, &plan()).contains("measured:4 is outside repetitions 1..=3"));
}

#[test]
fn missing_or_replaced_case_fails() {
    // A missing case with the count still checked.
    let rows = complete_rows()[..8].to_vec();
    let message = error_of(&rows, &plan());
    assert!(
        message.contains("missing [\"10002\"]; not in the manifest []"),
        "{message}"
    );
    // The same total with one case swapped for a case outside the corpus.
    let mut rows = complete_rows()[..8].to_vec();
    rows.extend(executed("99999", "passed"));
    let message = error_of(&rows, &plan());
    assert!(
        message.contains("missing [\"10002\"]; not in the manifest [\"99999\"]"),
        "{message}"
    );
    // Without a manifest the count alone must match.
    let count_only = RunPlan {
        case_manifest: None,
        ..plan()
    };
    assert!(
        error_of(&complete_rows()[..8], &count_only).contains("the run holds 2 cases, 3 expected")
    );
    // A manifest that disagrees with --expected-cases is refused.
    let wrong = RunPlan {
        expected_cases: 4,
        ..plan()
    };
    assert!(error_of(&complete_rows(), &wrong).contains("manifest lists 3 cases, but 4"));
}

#[test]
fn empty_or_shapeless_output_fails() {
    assert!(error_of(&[], &plan()).contains("the run's 0 cases are not the manifest's 3"));
    let count_only = RunPlan {
        case_manifest: None,
        ..plan()
    };
    assert!(error_of(&[], &count_only).contains("the run holds 0 cases, 3 expected"));
    // An empty file is refused as such, marker or not.
    let dir = tempfile::tempdir().unwrap();
    let raw = write_run(dir.path(), &[]);
    let message = format!("{:#}", validate_run_path(&raw, &plan()).unwrap_err());
    assert!(message.contains("is empty"), "{message}");
    let braces = format!("{:#}", validate(&["{}".to_owned()], &plan()).unwrap_err());
    assert!(braces.contains("line 1: missing case_id"), "{braces}");
}

#[test]
fn completion_marker_must_certify_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let raw = write_run(dir.path(), &complete_rows());
    let validation = validate_run_path(&raw, &plan()).expect("complete run with its marker");
    assert_eq!(validation.records, 12);

    // An interrupted run leaves no marker.
    fs::remove_file(completion_marker_path(&raw)).unwrap();
    let message = format!("{:#}", validate_run_path(&raw, &plan()).unwrap_err());
    assert!(message.contains("wrote no completion marker"), "{message}");

    // A marker for other bytes does not certify these.
    let raw = write_run(dir.path(), &complete_rows());
    let mut text = fs::read_to_string(&raw).unwrap();
    text = text.replacen("\"target_elapsed_ns\":900", "\"target_elapsed_ns\":100", 1);
    fs::write(&raw, text).unwrap();
    let message = format!("{:#}", validate_run_path(&raw, &plan()).unwrap_err());
    assert!(message.contains("records raw_sha256"), "{message}");

    // A truncated last line is a parse error before any marker is read.
    let raw = write_run(dir.path(), &complete_rows());
    let text = fs::read_to_string(&raw).unwrap();
    fs::write(&raw, &text[..text.len() - 20]).unwrap();
    let message = format!("{:#}", validate_run_path(&raw, &plan()).unwrap_err());
    assert!(message.contains("JSONL line 12: not JSON"), "{message}");
}

#[test]
fn case_manifest_reads_runner_listing_ids() {
    let ids = parse_case_manifest(r#"[{"id": 76, "name": "A"}, {"id": 10000, "name": "B"}]"#)
        .expect("listing");
    assert_eq!(
        ids.into_iter().collect::<Vec<_>>(),
        ["00076".to_owned(), "10000".to_owned()]
    );
    assert!(parse_case_manifest("[]").is_err());
    assert!(parse_case_manifest(r#"[{"id": 1}, {"id": 1}]"#).is_err());
    assert!(parse_case_manifest(r#"{"id": 1}"#).is_err());
    assert!(parse_case_manifest(r#"[{"id": "00001"}]"#).is_err());
}
