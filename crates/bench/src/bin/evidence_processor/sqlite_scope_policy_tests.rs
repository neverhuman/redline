//! The SQLite scope policy: how it parses, that the run recorded it, and
//! every way a suite's skips can differ from it.

use super::*;

fn policy(entries: &[(&str, &str)]) -> ScopePolicy {
    let exceptions = entries
        .iter()
        .map(|(suite, case_id)| {
            json!({"suite": suite, "case_id": case_id, "name": "N", "kind": "rql_rewrite",
                   "reason": "r", "owner": "o", "expiry": "2027-03-31"})
        })
        .collect::<Vec<_>>();
    let text =
        json!({"schema_version": POLICY_SCHEMA, "description": "d", "exceptions": exceptions});
    ScopePolicy::parse(text.to_string().as_bytes()).expect("policy")
}

fn entry(skipped: &[&str]) -> Value {
    json!({"skipped": skipped.len(), "skipped_case_ids": skipped})
}

fn ids(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

#[test]
fn skip_ids_must_equal_policy() {
    let policy = policy(&[("rql_phase1", "00007"), ("rql_phase1", "00010")]);
    let listed = ids(&["00007", "00010"]);
    policy
        .check_suite("rql_phase1", &entry(&["00007", "00010"]), &listed)
        .expect("exactly the listed skips");
    // The old budget accepted four anonymous sqlite_parity skips.
    let four = ids(&["00093", "00094", "00095", "00096"]);
    let error = policy
        .check_suite(
            "sqlite_parity",
            &entry(&["00093", "00094", "00095", "00096"]),
            &four,
        )
        .expect_err("unlisted skips");
    assert!(
        format!("{error:#}").contains("skipped but not listed"),
        "{error:#}"
    );
    // A listed case that ran.
    let error = policy
        .check_suite("rql_phase1", &entry(&["00007"]), &ids(&["00007"]))
        .expect_err("listed case ran");
    assert!(
        format!("{error:#}").contains("listed but not skipped [\"00010\"]"),
        "{error:#}"
    );
    // One more skip than the policy allows.
    let three = ids(&["00007", "00010", "00011"]);
    assert!(
        policy
            .check_suite("rql_phase1", &entry(&["00007", "00010", "00011"]), &three)
            .is_err()
    );
    // The runner's declaration must match its raw records.
    assert!(
        policy
            .check_suite("rql_phase1", &entry(&["00007", "00011"]), &listed)
            .is_err()
    );
    let mut miscounted = entry(&["00007", "00010"]);
    miscounted["skipped"] = json!(3);
    assert!(
        policy
            .check_suite("rql_phase1", &miscounted, &listed)
            .is_err()
    );
    assert!(
        policy
            .check_suite("rql_phase1", &json!({"skipped": 2}), &listed)
            .is_err()
    );
}

#[test]
fn the_run_must_record_the_committed_policy() {
    let policy = policy(&[("rql_phase1", "00007")]);
    let official = json!({"sqlite_scope_policy": {
        "schema_version": POLICY_SCHEMA, "path": "x", "sha256": policy.sha256}});
    policy.check_recorded(&official).expect("same file");
    let mut stale = official.clone();
    stale["sqlite_scope_policy"]["sha256"] = json!("0".repeat(64));
    assert!(policy.check_recorded(&stale).is_err());
    assert!(policy.check_recorded(&json!({})).is_err());
}

#[test]
fn raw_skips_must_name_their_exception() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("raw.jsonl");
    let write = |text: &str| fs::write(&path, text).expect("raw");
    write(concat!(
        "{\"case_id\":\"00007\",\"status\":\"skipped\",\"policy_exception_id\":\"rql_phase1:00007\"}\n",
        "{\"case_id\":\"00008\",\"status\":\"passed\"}\n",
    ));
    assert_eq!(
        raw_skipped_case_ids(&path, "rql_phase1").expect("raw"),
        ids(&["00007"])
    );
    assert!(raw_skipped_case_ids(&path, "memory").is_err());
    write("{\"case_id\":\"00007\",\"status\":\"skipped\",\"policy_exception_id\":null}\n");
    assert!(raw_skipped_case_ids(&path, "rql_phase1").is_err());
}

#[test]
fn committed_policy_is_well_formed() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let policy = ScopePolicy::load(&repo_root).expect("committed policy");
    assert!(policy.by_suite["sqlite_parity"].is_empty());
    assert!(policy.by_suite["memory"].is_empty());
    assert!(!policy.by_suite["rql_phase1"].is_empty());
}

#[test]
fn malformed_policies_are_rejected() {
    for exceptions in [
        json!([{"suite": "beyond_sqlite", "case_id": "1", "name": "N", "kind": "k", "reason": "r", "owner": "o", "expiry": "e"}]),
        json!([{"suite": "memory", "case_id": "00001", "name": "N", "kind": "k", "reason": "", "owner": "o", "expiry": "e"}]),
        json!([
            {"suite": "memory", "case_id": "00001", "name": "N", "kind": "k", "reason": "r", "owner": "o", "expiry": "e"},
            {"suite": "memory", "case_id": "00001", "name": "N", "kind": "k", "reason": "r", "owner": "o", "expiry": "e"}
        ]),
    ] {
        let text = json!({"schema_version": POLICY_SCHEMA, "exceptions": exceptions});
        assert!(
            ScopePolicy::parse(text.to_string().as_bytes()).is_err(),
            "{text}"
        );
    }
    assert!(ScopePolicy::parse(br#"{"schema_version":"v0","exceptions":[]}"#).is_err());
}
