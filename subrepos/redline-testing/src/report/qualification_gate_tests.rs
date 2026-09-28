//! The SQLite qualification gate against the runner's own counts
//! (L-01/SQ-01, SQ-04).
//!
//! Each Unqualified branch of `qualify()` is driven with an otherwise clean
//! 3/3 run, so a deleted gate would turn the badge green, and a case the
//! runner counts as failed (any sample failed) must render red with counts
//! that add up.

use std::fs;

use super::generate;
use super::qualification_tests::{
    README, badge, evidence, failed, options, passed, record, render, report_block, skipped,
    three_passed,
};
use super::utils::sha256_hex;

/// A case whose first and third measured samples passed and whose second
/// failed: the runner counts it failed.
fn flaky(case_id: &str, name: &str) -> String {
    [
        record(case_id, name, "measured:1", Some(1), "passed"),
        record(case_id, name, "measured:2", Some(2), "failed"),
        record(case_id, name, "measured:3", Some(3), "passed"),
    ]
    .concat()
}

#[test]
fn a_case_failing_one_sample_is_a_red_failure_not_unqualified() {
    // The runner's counts: 00200 failed (one sample failed), 00300 skipped.
    let raw = [
        passed("00001", "PLAIN"),
        flaky("00200", "FLAKY"),
        skipped("00300", "SKIPPED"),
    ]
    .concat();
    let readme = render(&raw, Some(evidence(&raw, "failed", [3, 1, 1, 1])), README);
    let badge = badge(&readme);
    assert!(
        badge.contains(
            "-1%2F3%20%C2%B7%201%20failed%20%C2%B7%201%20skipped%20%C2%B7%203.53.1-red\""
        ),
        "{badge}"
    );
    let block = report_block(&readme);
    assert!(
        block.contains("**1 / 3** cases passed, **1** failed, **1** skipped."),
        "{block}"
    );
    assert!(
        block.contains("**Evidence:** qualified: official evidence run `daac7524c769` records the same 3 total, 1 passed, 1 failed, 1 skipped."),
        "{block}"
    );
}

#[test]
fn a_case_failing_only_its_warmup_is_a_red_failure() {
    let root = tempfile::tempdir().expect("temp root");
    let warm = |case_id: &str, status: &str| {
        let mut rows = record(case_id, "WARM", "warmup", None, status);
        rows.push_str(&passed(case_id, "WARM"));
        rows
    };
    let raw = [warm("00001", "passed"), warm("00002", "failed")].concat();
    let value = evidence(&raw, "failed", [2, 1, 1, 0]);
    let evidence_path = root.path().join("official-evidence.processed.json");
    fs::write(&evidence_path, value.to_string()).expect("evidence");
    fs::write(root.path().join("raw.jsonl"), &raw).expect("raw");
    fs::write(root.path().join("README.md"), README).expect("readme");
    let mut options = options(root.path(), Some(evidence_path), &raw);
    options.expected_warmup = Some(1);
    generate(options).expect("report");
    let readme = fs::read_to_string(root.path().join("README.md")).expect("readme");
    let badge = badge(&readme);
    assert!(
        badge.contains(
            "-1%2F2%20%C2%B7%201%20failed%20%C2%B7%200%20skipped%20%C2%B7%203.53.1-red\""
        ),
        "{badge}"
    );
}

#[test]
fn incomplete_evidence_never_qualifies_a_clean_run() {
    let raw = three_passed();
    let clean = || evidence(&raw, "passed", [3, 3, 0, 0]);
    let mut no_status = clean();
    no_status.as_object_mut().unwrap().remove("status");
    no_status["official_evidence"]
        .as_object_mut()
        .unwrap()
        .remove("status");
    let mut running = clean();
    running["status"] = "running".into();
    let mut no_counts = clean();
    for key in ["total", "passed", "failed", "skipped"] {
        no_counts["suite_summaries"]["sqlite_parity"]
            .as_object_mut()
            .unwrap()
            .remove(key);
    }
    // A version line with no release number in it: the raw samples name
    // the same line, so only the oracle release is missing.
    let unversioned = "sqlite3 custom build (64-bit)";
    let unversioned_raw = raw.replace(super::test_fixtures::SQLITE_VERSION, unversioned);
    let mut no_oracle = evidence(&unversioned_raw, "passed", [3, 3, 0, 0]);
    no_oracle["official_evidence"]["sqlite"]["version"] = unversioned.into();
    let mut passed_with_failures = evidence(
        &[three_passed(), failed("00200", "BROKEN")].concat(),
        "passed",
        [4, 3, 1, 0],
    );
    passed_with_failures["official_evidence"]["status"] = "passed".into();
    for (what, raw, value, reason) in [
        (
            "no status",
            raw.clone(),
            no_status,
            "official evidence records no run status",
        ),
        (
            "status running",
            raw.clone(),
            running,
            "official evidence status is \"running\"",
        ),
        (
            "no counts",
            raw.clone(),
            no_counts,
            "official evidence records no sqlite_parity counts",
        ),
        (
            "no oracle release",
            unversioned_raw,
            no_oracle,
            "official evidence records no SQLite oracle version",
        ),
        (
            "passed with a failure",
            [three_passed(), failed("00200", "BROKEN")].concat(),
            passed_with_failures,
            "official evidence status is passed, but the raw results have 1 failed",
        ),
    ] {
        let readme = render(&raw, Some(value), README);
        assert_unqualified(&readme, reason, what);
    }
}

fn assert_unqualified(readme: &str, reason: &str, what: &str) {
    let badge = badge(readme);
    assert!(
        badge.contains("corpus-unqualified-lightgrey") && !badge.contains("brightgreen"),
        "{what}: {badge}"
    );
    let block = report_block(readme);
    assert!(
        block.contains(&format!("**Evidence:** unqualified: {reason}.")),
        "{what}: missing reason {reason:?} in:\n{block}"
    );
}

fn render_result(raw: &str, value: serde_json::Value) -> anyhow::Result<String> {
    let root = tempfile::tempdir().expect("temp root");
    let evidence_path = root.path().join("official-evidence.processed.json");
    fs::write(&evidence_path, value.to_string()).expect("evidence");
    fs::write(root.path().join("raw.jsonl"), raw).expect("raw");
    fs::write(root.path().join("README.md"), README).expect("readme");
    generate(options(root.path(), Some(evidence_path), raw))?;
    Ok(fs::read_to_string(root.path().join("README.md")).expect("readme"))
}

#[test]
fn unprocessed_run_evidence_qualifies_under_its_own_hash() {
    // The runner's own official-evidence.json: counts under
    // suites.sqlite_parity, the raw hash under output_file_hashes, and no
    // source_sha256, so the file itself is the run's identity.
    let raw = three_passed();
    let processed = evidence(&raw, "passed", [3, 3, 0, 0]);
    let mut run = processed["official_evidence"].clone();
    run["suites"] = serde_json::json!({
        "sqlite_parity": {
            "total": 3, "passed": 3, "failed": 0, "skipped": 0,
            "raw_path": "sqlite_parity.raw.jsonl",
        }
    });
    run["output_file_hashes"] = serde_json::json!({ "sqlite_parity.raw.jsonl": sha256_hex(&raw) });
    // render_result writes the evidence as `run.to_string()`.
    let run_id = sha256_hex(&run.to_string());
    let readme = render_result(&raw, run).expect("report from unprocessed evidence");
    let badge = badge(&readme);
    assert!(badge.contains("-brightgreen\""), "{badge}");
    let block = report_block(&readme);
    assert!(
        block.contains(&format!(
            "**Evidence:** qualified: official evidence run `{}` records the same 3 total, 3 passed, 0 failed, 0 skipped.",
            &run_id[..12]
        )),
        "{block}"
    );
}
