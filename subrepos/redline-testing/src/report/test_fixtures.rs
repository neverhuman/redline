//! Shared fixtures for report tests: raw records, processed official
//! evidence and run provenance that describe one consistent run.

use serde_json::{Value, json};

use super::utils::sha256_hex;

pub(crate) const RUNNER_PATH: &str = "/ci/target/release/redline-testing";
pub(crate) const RUNNER_SHA256: &str =
    "b28c41d40009bfe7c98832abda695c3b9d2624871c4c886991601718d8e70a78";
pub(crate) const RUNNER_VERSION: &str = "redline-testing 1.0.1";
pub(crate) const TARGET_PATH: &str = "/ci/target/release/redlinedb";
pub(crate) const TARGET_SHA256: &str =
    "be49779bace1d97c1d9a78f697f3a7b2bd27c6c4c6a5dccefd29304dfaa55b78";
pub(crate) const TARGET_VERSION: &str = "redlinedb v4.1.0 (SQLite 3.45.1 compatibility)";
pub(crate) const SQLITE_PATH: &str = "/ci/target/sqlite-reference/3.53.1/bin/sqlite3";
pub(crate) const SQLITE_SHA256: &str =
    "e99d817b62f1ad9ead02b8d4e410fea9d736ee82c35b9a7daa6644d4d3e5ae3a";
pub(crate) const SQLITE_VERSION: &str = "3.53.1 2026-05-05 10:34:17 c88b22011a54 (64-bit)";
pub(crate) const SOURCE_COMMIT: &str = "c1af369c1af369c1af369c1af369c1af369c1af3";
pub(crate) const SOURCE_TREE: &str = "7ee7ee7ee7ee7ee7ee7ee7ee7ee7ee7ee7ee7ee7";
pub(crate) const SOURCE_INPUTS_SHA256: &str =
    "cf704799b75ce7408d3ce1adec3cf7e93aa1ca9f25a6dbbce1429782bf34e22f";
pub(crate) const CORPUS_SHA256: &str =
    "c0ffeec0ffeec0ffeec0ffeec0ffeec0ffeec0ffeec0ffeec0ffeec0ffeec0ff";
pub(crate) const ASSERTION_POLICY_SHA256: &str =
    "a55e47a55e47a55e47a55e47a55e47a55e47a55e47a55e47a55e47a55e47a55e";
pub(crate) const ORACLE_BUILD_STAMP: &str =
    "36ca143645cf76997d07b66e9244c636b8ccdec64a1d50558259c4e415e6558b\n-O2";
pub(crate) const RUN_ELAPSED_NS: u64 = 1_395_000_000;
pub(crate) const RUN_PROVENANCE_SCHEMA: &str = "redline-testing-run-provenance-v2";

/// One passed case measured three times, with the executable identities the
/// runner records on every sample.
pub(crate) fn raw_case(case_id: &str) -> String {
    (1..=3usize)
        .map(|repetition| {
            let record = json!({
                "case_id": case_id,
                "name": format!("CASE_{case_id}"),
                "case_file": format!("CASE_{case_id}.rs"),
                "priority": "P0",
                "profile": "memory",
                "category": "SMOKE",
                "sample_role": format!("measured:{repetition}"),
                "repetition_index": repetition,
                "status": "passed",
                "reference_executable_path": SQLITE_PATH,
                "target_executable_path": TARGET_PATH,
                "reference_executable_sha256": SQLITE_SHA256,
                "target_executable_sha256": TARGET_SHA256,
                "reference_version": SQLITE_VERSION,
                "target_version": TARGET_VERSION,
                "reference_elapsed_ns": 2_000_000u64,
                "target_elapsed_ns": 4_000_000u64,
            });
            format!("{record}\n")
        })
        .collect()
}

/// The run's own identity fields, as the runner records them both in
/// official-evidence.json and in the run provenance.
fn run_identity() -> Value {
    json!({
        "run_provenance_schema": RUN_PROVENANCE_SCHEMA,
        "source_commit": SOURCE_COMMIT,
        "source_tree": SOURCE_TREE,
        "source_inputs_sha256": SOURCE_INPUTS_SHA256,
        "source_dirty": false,
        "corpus_sha256": CORPUS_SHA256,
        "assertion_policy_sha256": ASSERTION_POLICY_SHA256,
        "oracle_build_stamp": ORACLE_BUILD_STAMP,
    })
}

/// Processed official evidence for a run that recorded no run provenance
/// (the shape of the committed 2026-09-24 run).
pub(crate) fn historical_evidence(raw_text: &str) -> Value {
    let raw_sha256 = sha256_hex(raw_text);
    let cases = raw_text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|record| record["case_id"].as_str().map(str::to_owned))
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    json!({
        "schema_version": "redline-testing-official-evidence-processed-v1",
        "source_sha256": "daac7524c76944c99fdaf6ac397034c9aa5634906f035771ba83d7fd10e54ccc",
        "status": "passed",
        "target": { "path": TARGET_PATH, "sha256": TARGET_SHA256, "version": TARGET_VERSION },
        "sqlite": { "path": SQLITE_PATH, "sha256": SQLITE_SHA256, "version": SQLITE_VERSION },
        "official_evidence": {
            "schema_version": "redline-testing-official-evidence-v1",
            "runner": {
                "binary_path": RUNNER_PATH,
                "binary_sha256": RUNNER_SHA256,
                "release_binary_sha256": RUNNER_SHA256,
                "version": RUNNER_VERSION,
            },
            "target": { "path": TARGET_PATH, "sha256": TARGET_SHA256, "version": TARGET_VERSION },
            "sqlite": { "path": SQLITE_PATH, "sha256": SQLITE_SHA256, "version": SQLITE_VERSION },
            "status": "passed",
            "command_line": [RUNNER_PATH, "run", "--suite", "all", "--workers", "auto"],
            "workers": "128",
        },
        "suite_summaries": {
            "sqlite_parity": {
                "total": cases,
                "passed": cases,
                "failed": 0,
                "skipped": 0,
                "raw_path": "sqlite_parity.raw.jsonl",
                "provenance_path": "provenance.json",
                "raw_sha256": raw_sha256,
                "provenance_sha256": "8de6a84800000000000000000000000000000000000000000000000000000000",
            }
        },
    })
}

/// The run provenance the runner writes next to `sqlite_parity.raw.jsonl`.
pub(crate) fn run_provenance(raw_text: &str) -> Value {
    let mut value = json!({
        "schema_version": RUN_PROVENANCE_SCHEMA,
        "suite": "sqlite_parity",
        "target_binary_path": TARGET_PATH,
        "target_binary_sha256": TARGET_SHA256,
        "target_version": TARGET_VERSION,
        "redline_testing_binary_path": RUNNER_PATH,
        "redline_testing_binary_sha256": RUNNER_SHA256,
        "redline_testing_version": RUNNER_VERSION,
        "sqlite_binary_path": SQLITE_PATH,
        "sqlite_binary_sha256": SQLITE_SHA256,
        "sqlite_version": SQLITE_VERSION,
        "source_dirty_paths": [],
        "elapsed_ns": RUN_ELAPSED_NS,
        "output_file_hashes": {
            "sqlite_parity.raw.jsonl": sha256_hex(raw_text),
        },
    });
    merge(&mut value, &run_identity());
    value
        .as_object_mut()
        .expect("object")
        .remove("run_provenance_schema");
    value
}

/// Official evidence for a run that wrote `run_provenance`, bound to it by
/// `provenance_sha256`, with the run's completion marker, exactly as the
/// evidence processor records them.
pub(crate) fn official_evidence(raw_text: &str, run_provenance_text: &str) -> Value {
    let mut value = historical_evidence(raw_text);
    merge(&mut value["official_evidence"], &run_identity());
    value["suite_summaries"]["sqlite_parity"]["provenance_sha256"] =
        sha256_hex(run_provenance_text).into();
    value["suite_summaries"]["sqlite_parity"]["completion"] = completion(raw_text);
    value
}

/// The completion marker a finished run writes beside `raw_text`.
pub(crate) fn completion(raw_text: &str) -> Value {
    let records = raw_text.lines().filter(|line| !line.trim().is_empty());
    let cases = records
        .clone()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|record| record["case_id"].as_str().map(str::to_owned))
        .collect::<std::collections::BTreeSet<_>>();
    json!({
        "schema_version": "redline-testing-raw-complete-v1",
        "suite": "sqlite_parity",
        "raw_file": "sqlite_parity.raw.jsonl",
        "records": records.count(),
        "cases": cases.len(),
        "raw_sha256": sha256_hex(raw_text),
    })
}

/// The case ids in `raw_text`: the manifest of a fixture run.
pub(crate) fn case_ids(raw_text: &str) -> std::collections::BTreeSet<String> {
    raw_text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|record| record["case_id"].as_str().map(str::to_owned))
        .collect()
}

pub(crate) fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("fixture json") + "\n"
}

fn merge(target: &mut Value, fields: &Value) {
    let target = target.as_object_mut().expect("fixture object");
    for (key, value) in fields.as_object().expect("fixture fields") {
        target.insert(key.clone(), value.clone());
    }
}
