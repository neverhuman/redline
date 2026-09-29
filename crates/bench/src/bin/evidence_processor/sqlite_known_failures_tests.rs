//! The SQLite known-failures baseline: how it parses, that the run recorded
//! it, and every way a suite's failures can differ from it.

use super::*;

fn baseline(entries: &[(&str, &str)]) -> Baseline {
    let failures = entries
        .iter()
        .map(|(suite, case_id)| {
            json!({"suite": suite, "case_id": case_id, "name": "N",
                   "stage": "target_semantic_failure", "reason": "r", "owner": "o"})
        })
        .collect::<Vec<_>>();
    let text = json!({"schema_version": BASELINE_SCHEMA, "description": "d", "failures": failures});
    Baseline::parse(text.to_string().as_bytes()).expect("baseline")
}

fn entry(failed: &[&str], listed: &[&str]) -> Value {
    json!({"failed": failed.len(), "failed_case_ids": failed, "known_failure_ids": listed})
}

fn ids(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

/// Raw failures, each failing with the given verdict_reasons.
fn failures(cases: &[(&str, &[&str])]) -> RawFailures {
    cases
        .iter()
        .map(|(case_id, reasons)| ((*case_id).to_owned(), ids(reasons)))
        .collect()
}

/// Raw failures, each a target semantic failure (the fixture stage).
fn failed(case_ids: &[&str]) -> RawFailures {
    case_ids
        .iter()
        .map(|case_id| ((*case_id).to_owned(), ids(&["target_semantic_failure"])))
        .collect()
}

#[test]
fn sqlite_failures_must_equal_the_known_failures_baseline() {
    let known = baseline(&[("sqlite_parity", "10547"), ("memory", "10547")]);
    let raw = failed(&["10547"]);
    known
        .check_suite("sqlite_parity", &entry(&["10547"], &["10547"]), &raw)
        .expect("listed failure");
    known
        .check_suite("memory", &entry(&["10547"], &["10547"]), &raw)
        .expect("listed failure");
    // A failure the baseline does not list.
    let raw = failed(&["10547", "00076"]);
    let error = known
        .check_suite(
            "sqlite_parity",
            &entry(&["00076", "10547"], &["10547"]),
            &raw,
        )
        .expect_err("unlisted failure");
    assert!(
        format!("{error:#}").contains("failed but not listed [\"00076\"]"),
        "{error:#}"
    );
    // A listed case that passed.
    let error = known
        .check_suite("sqlite_parity", &entry(&[], &["10547"]), &failed(&[]))
        .expect_err("listed pass");
    assert!(
        format!("{error:#}").contains("remove those from the baseline"),
        "{error:#}"
    );
}

#[test]
fn declared_failures_must_match_the_raw_records() {
    let known = baseline(&[("sqlite_parity", "10547")]);
    // The summary hides a failure the raw records show.
    let error = known
        .check_suite(
            "sqlite_parity",
            &entry(&[], &["10547"]),
            &failed(&["10547"]),
        )
        .expect_err("hidden failure");
    assert!(
        format!("{error:#}").contains("raw records fail 1"),
        "{error:#}"
    );
    // The run was gated with a different list than the committed one.
    let error = known
        .check_suite(
            "sqlite_parity",
            &entry(&["10547"], &[]),
            &failed(&["10547"]),
        )
        .expect_err("different baseline");
    assert!(
        format!("{error:#}").contains("was gated with known failures"),
        "{error:#}"
    );
    // No declared lists at all.
    assert!(
        known
            .check_suite("sqlite_parity", &json!({"failed": 1}), &failed(&["10547"]))
            .is_err()
    );
}

#[test]
fn the_run_must_record_the_committed_baseline() {
    let known = baseline(&[("memory", "00134")]);
    let official = json!({"sqlite_known_failures": {
        "schema_version": BASELINE_SCHEMA, "path": "x", "sha256": known.sha256}});
    known.check_recorded(&official).expect("same file");
    let mut stale = official.clone();
    stale["sqlite_known_failures"]["sha256"] = json!("0".repeat(64));
    assert!(known.check_recorded(&stale).is_err());
    assert!(known.check_recorded(&json!({})).is_err());
}

#[test]
fn malformed_baselines_are_rejected() {
    for failures in [
        json!([{"suite": "rql_phase1", "case_id": "1", "name": "N", "stage": "s", "reason": "r", "owner": "o"}]),
        json!([{"suite": "memory", "case_id": "1", "name": "N", "stage": "s", "reason": "", "owner": "o"}]),
        json!([
            {"suite": "memory", "case_id": "1", "name": "N", "stage": "differential_mismatch", "reason": "r", "owner": "o"},
            {"suite": "memory", "case_id": "1", "name": "N", "stage": "differential_mismatch", "reason": "r", "owner": "o"}
        ]),
        // A reference contract failure is a corpus defect, never a
        // baseline entry.
        json!([{"suite": "memory", "case_id": "1", "name": "N", "stage": "reference_contract_failure", "reason": "r", "owner": "o"}]),
    ] {
        let text = json!({"schema_version": BASELINE_SCHEMA, "failures": failures});
        assert!(
            Baseline::parse(text.to_string().as_bytes()).is_err(),
            "{text}"
        );
    }
    assert!(Baseline::parse(br#"{"schema_version":"v0","failures":[]}"#).is_err());
}

#[test]
fn committed_baseline_is_well_formed() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let known = Baseline::load(&repo_root).expect("committed baseline");
    // sqlite_parity and memory run the same corpus through the same
    // comparator, so they fail the same cases.
    assert_eq!(known.by_suite["sqlite_parity"], known.by_suite["memory"]);
}

#[test]
fn a_listed_failure_must_fail_at_its_listed_stage() {
    // 10547 is listed as target_semantic_failure. A run that fails it
    // another way was rejected by the runner's gate, but its evidence
    // was written first, so the processor must reject it too.
    let known = baseline(&[("sqlite_parity", "10547")]);
    let entry = entry(&["10547"], &["10547"]);
    known
        .check_suite(
            "sqlite_parity",
            &entry,
            &failures(&[("10547", &["target_semantic_failure"])]),
        )
        .expect("listed stage");
    for reasons in [
        &["reference_contract_failure"][..],
        &["differential_mismatch"],
        &["target_semantic_failure", "execution_failure"],
        &["unrecorded"],
    ] {
        let error = known
            .check_suite("sqlite_parity", &entry, &failures(&[("10547", reasons)]))
            .expect_err("another stage");
        assert!(
            format!("{error:#}").contains(
                "suite sqlite_parity case 10547 is listed as target_semantic_failure, but its raw records fail as"
            ),
            "{reasons:?}: {error:#}"
        );
    }
}

#[test]
fn raw_failures_come_from_failed_records() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("raw.jsonl");
    fs::write(
        &path,
        concat!(
            "{\"case_id\":\"00001\",\"status\":\"passed\",\"verdict_reason\":\"passed\"}\n",
            "{\"case_id\":\"00002\",\"status\":\"failed\",\"verdict_reason\":\"differential_mismatch\"}\n",
            "{\"case_id\":\"00002\",\"status\":\"passed\",\"verdict_reason\":\"passed\"}\n",
            "{\"case_id\":\"00003\",\"status\":\"skipped\",\"verdict_reason\":\"skipped\"}\n",
            "{\"case_id\":\"00004\",\"status\":\"failed\",\"verdict_reason\":\"target_semantic_failure\"}\n",
            "{\"case_id\":\"00004\",\"status\":\"failed\",\"verdict_reason\":\"execution_failure\"}\n",
            "{\"case_id\":\"00005\",\"status\":\"failed\"}\n",
        ),
    )
    .expect("write raw");
    assert_eq!(
        raw_failures(&path).expect("raw"),
        failures(&[
            ("00002", &["differential_mismatch"]),
            ("00004", &["execution_failure", "target_semantic_failure"]),
            ("00005", &["unrecorded"]),
        ])
    );
}
