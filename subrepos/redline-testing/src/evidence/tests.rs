//! Runner-side evidence tests: the ranked.csv written during `run` must use
//! the same unfloored latency ratio as the `report` renderer (L-04/R4-01).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{EvidenceConfig, write_sqlite_parity_evidence};
use crate::sqlite_parity::case::{Case, Priority, Profile};
use crate::sqlite_parity::{CaseFailure, RunSummary, VerdictReason};

#[cfg(unix)]
fn fake_bin(dir: &Path, name: &str, version: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' {version:?}\n")).expect("bin");
    let mut permissions = fs::metadata(&path).expect("bin metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("bin permissions");
    path
}

fn raw_case(case_id: &str, reference_ns: u128, target_ns: u128) -> String {
    let mut out = String::new();
    for repetition in 1..=3usize {
        let record = serde_json::json!({
            "case_id": case_id,
            "name": format!("CASE_{case_id}"),
            "case_file": format!("{case_id}.sql"),
            "priority": "P0",
            "profile": "memory",
            "category": "SMOKE",
            "sample_role": format!("measured:{repetition}"),
            "repetition_index": repetition,
            "status": "passed",
            "reference_elapsed_ns": reference_ns,
            "target_elapsed_ns": target_ns,
        });
        out.push_str(&serde_json::to_string(&record).expect("raw json"));
        out.push('\n');
    }
    out
}

#[cfg(unix)]
fn write_evidence(raw_text: &str) -> (tempfile::TempDir, String, String) {
    write_evidence_with(
        raw_text,
        RunSummary {
            total: 2,
            passed: 2,
            failed: 0,
            skipped: 0,
            elapsed: Duration::from_millis(5),
            slowest: Vec::new(),
            failures: Vec::new(),
            skipped_case_ids: Vec::new(),
            passed_case_ids: Vec::new(),
        },
    )
}

#[cfg(unix)]
fn write_evidence_with(raw_text: &str, summary: RunSummary) -> (tempfile::TempDir, String, String) {
    let root = tempfile::Builder::new()
        .prefix("redline-testing-runner-evidence-")
        .tempdir()
        .expect("temp root");
    let output = root.path().join("sqlite_parity.raw.jsonl");
    fs::write(&output, raw_text).expect("raw");
    let target_bin = fake_bin(root.path(), "redlinedb", "redlinedb test");
    let sqlite_bin = fake_bin(root.path(), "sqlite3", "3.53.1 test");
    write_sqlite_parity_evidence(EvidenceConfig {
        suite: "sqlite_parity".to_owned(),
        output,
        target_bin,
        sqlite_bin: sqlite_bin.clone(),
        tmp_root: root.path().join("tmp"),
        workers: "4".to_owned(),
        repetitions: 3,
        warmup: 0,
        memory_samples: false,
        measurement_order: Default::default(),
        command_line: vec!["redline-testing".to_owned(), "run".to_owned()],
        started_unix_ms: 1,
        ended_unix_ms: 2,
        summary,
        run_identity: super::identity::capture(&sqlite_bin),
    })
    .expect("write runner evidence");
    let ranked = fs::read_to_string(root.path().join("ranked.csv")).expect("ranked.csv");
    let summary = fs::read_to_string(root.path().join("summary.json")).expect("summary.json");
    (root, ranked, summary)
}

fn csv_row<'a>(ranked_csv: &'a str, case_id: &str) -> &'a str {
    ranked_csv
        .lines()
        .find(|line| line.split(',').nth(1) == Some(case_id))
        .unwrap_or_else(|| panic!("no ranked.csv row for {case_id}:\n{ranked_csv}"))
}

#[cfg(unix)]
#[test]
fn runner_ranked_csv_has_no_reference_floor() {
    let raw = format!(
        "{}{}",
        raw_case("00001", 1_000_000, 2_000_000),
        raw_case("00011", 2_916_608, 612_825_241)
    );
    let (_root, ranked, summary) = write_evidence(&raw);
    assert!(
        ranked.starts_with(crate::latency::RANKED_CSV_HEADER),
        "{ranked}"
    );
    let summary: serde_json::Value = serde_json::from_str(&summary).expect("summary json");
    assert_eq!(summary["measurement_boundary"], "cli_case_wall_time");
    assert_eq!(summary["ranked_schema"], "redline-testing-ranked-v2");
    let sub_ms = csv_row(&ranked, "00001");
    assert!(
        !sub_ms.contains("33.333333"),
        "3 ms floor still applied: {sub_ms}"
    );
    assert!(sub_ms.contains(",2.000000,-100.000000,"), "{sub_ms}");
    assert!(
        sub_ms.ends_with(",true,3"),
        "below_resolution flag: {sub_ms}"
    );
    let trigger = csv_row(&ranked, "00011");
    assert!(trigger.contains(",210.115738,-20911.573753,"), "{trigger}");
    assert!(
        trigger.starts_with("1,"),
        "largest ratio ranks first: {trigger}"
    );
}

#[cfg(unix)]
#[test]
fn run_provenance_records_the_measured_identity() {
    let (root, _ranked, _summary) = write_evidence(&raw_case("00001", 1_000_000, 2_000_000));
    let text = fs::read_to_string(root.path().join("provenance.json")).expect("provenance");
    let provenance: serde_json::Value = serde_json::from_str(&text).expect("provenance json");
    assert_eq!(provenance["schema_version"], super::RUN_PROVENANCE_SCHEMA);
    assert_eq!(
        provenance["corpus_sha256"],
        crate::sqlite_parity::corpus_sha256()
    );
    assert_eq!(
        provenance["assertion_policy_sha256"],
        crate::sqlite_parity::assertion_policy_sha256()
    );
    // The suite's own elapsed time, not a placeholder zero.
    assert_eq!(provenance["elapsed_ns"], 5_000_000u64);
    assert_eq!(provenance["sqlite_version"], "3.53.1 test");
    assert_eq!(
        provenance["redline_testing_version"],
        format!("redline-testing {}", env!("CARGO_PKG_VERSION"))
    );
    // The test runs inside this checkout, so the source is identified.
    for key in ["source_commit", "source_tree", "source_inputs_sha256"] {
        assert!(provenance[key].is_string(), "{key}: {text}");
    }
    assert!(provenance["source_dirty"].is_boolean(), "{text}");
    assert!(provenance["source_dirty_paths"].is_array(), "{text}");
    // This sqlite3 has no build stamp beside it; official_tests covers a
    // stamped one.
    assert!(provenance["oracle_build_stamp"].is_null(), "{text}");
    assert!(provenance.get("redlinedb_git_dirty").is_none(), "{text}");
}

#[cfg(unix)]
#[test]
fn suite_summary_lists_failed_and_skipped_cases() {
    // Failures are published by id, beside the counts, so the evidence
    // processor can hold them to the known-failures baseline.
    let case = |id: usize| {
        serde_json::from_value::<Case>(serde_json::json!({
            "id": id, "folder": "F", "name": format!("CASE_{id:05}"), "category": "C",
            "priority": Priority::P0, "profile": Profile::Memory, "kind": "sql",
            "description": "", "status": "active", "db": ":memory:", "args": [],
            "stdin": "", "expected_exit": 0, "compare_stdout": true,
            "expected_stdout": null, "expected_stdout_contains": [],
            "expected_stderr_contains": [], "expected_combined_contains": [],
            "files": [], "script": null, "notes": ""
        }))
        .expect("case")
    };
    let mut summary = RunSummary::default();
    summary.record_run(case(1).display_id(), None);
    summary.record_run(
        case(11).display_id(),
        Some(CaseFailure {
            case_id: case(11).display_id(),
            name: case(11).name,
            verdict_reasons: [VerdictReason::TargetSemanticFailure].into(),
        }),
    );
    summary.record_skip(&case(21));
    let (_root, _ranked, summary) = write_evidence_with(
        &format!(
            "{}{}",
            raw_case("00001", 1_000_000, 2_000_000),
            raw_case("00011", 1_000_000, 2_000_000)
        ),
        summary,
    );
    let summary: serde_json::Value = serde_json::from_str(&summary).expect("summary json");
    assert_eq!(
        (
            &summary["total_cases"],
            &summary["passed_cases"],
            &summary["failed_cases"],
            &summary["skipped_cases"]
        ),
        (
            &serde_json::json!(3),
            &serde_json::json!(1),
            &serde_json::json!(1),
            &serde_json::json!(1)
        )
    );
    assert_eq!(summary["failed_case_ids"], serde_json::json!(["00011"]));
    assert_eq!(summary["skipped_case_ids"], serde_json::json!(["00021"]));
}
