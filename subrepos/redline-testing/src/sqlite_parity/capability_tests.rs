//! Capability gating fails closed (SQ-05): an unknown token and a probe
//! that cannot answer are errors, a reference without a declared
//! capability fails its case, and a target without one is skipped only
//! under a scope-policy exception.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::bounded::Limits;
use super::engine::{
    Capability, EngineSpec, ShellCapabilities, partition_cases, required_capabilities,
};
use super::runner::VerdictReason;
use super::scope_policy::{SCOPE_POLICY_SCHEMA, ScopePolicy};
use super::test_fixtures::plain_case;

fn every_capability(version: &str) -> ShellCapabilities {
    ShellCapabilities {
        version: version.to_owned(),
        percentile_functions: true,
        dot_crlf: true,
        dot_dbinfo: true,
        dot_dbtotxt: true,
        dot_recover: true,
        escape_symbol_option: true,
        fts5: true,
        rtree: true,
        dbstat: true,
        jsonb: true,
        math1: true,
        generate_series: true,
        json_pretty: true,
        jsonb_array_insert: true,
        regexp: true,
    }
}

fn policy_listing(suite: &str, case_id: &str) -> ScopePolicy {
    let text = serde_json::json!({
        "schema_version": SCOPE_POLICY_SCHEMA,
        "description": "d",
        "exceptions": [{
            "suite": suite, "case_id": case_id, "name": "UNIQUE_CONSTRAINT_FAILED",
            "kind": "target_capability", "reason": "r", "owner": "o", "expiry": "2099-12-31"
        }]
    });
    ScopePolicy::parse(text.to_string().as_bytes()).expect("policy")
}

fn empty_policy() -> ScopePolicy {
    let text = serde_json::json!({
        "schema_version": SCOPE_POLICY_SCHEMA, "description": "d", "exceptions": []
    });
    ScopePolicy::parse(text.to_string().as_bytes()).expect("policy")
}

fn regexp_case() -> super::case::Case {
    let mut case = plain_case();
    case.required_capabilities = vec!["regexp".to_owned()];
    case
}

#[test]
fn unknown_capability_token_is_error() {
    let mut case = plain_case();
    case.required_capabilities = vec!["REGEXP".to_owned()];
    let error = required_capabilities(&case).expect_err("an unknown token never runs ungated");
    assert!(
        format!("{error:#}").contains("unknown capability token \"REGEXP\""),
        "{error:#}"
    );
    assert_eq!(
        required_capabilities(&regexp_case()).expect("known token"),
        vec![Capability::Regexp]
    );
    // The partition refuses it too, whatever the shells support.
    let caps = every_capability("3.53.1");
    assert!(partition_cases(vec![case], &caps, &caps, &empty_policy(), "sqlite_parity").is_err());
}

#[test]
fn reference_capability_missing_fails_the_case() {
    let mut reference = every_capability("3.45.1");
    reference.regexp = false;
    let target = every_capability("redlinedb");
    // Even an exception for the case cannot turn it into a skip.
    let policy = policy_listing("sqlite_parity", "10547");
    let partition = partition_cases(
        vec![regexp_case()],
        &reference,
        &target,
        &policy,
        "sqlite_parity",
    )
    .expect("partition");
    assert!(partition.runnable.is_empty() && partition.skipped.is_empty());
    let rejected = &partition.rejected[0];
    assert_eq!(
        rejected.verdict_reason,
        VerdictReason::ReferenceCapabilityMissing
    );
    assert!(
        rejected
            .reason
            .contains("reference sqlite3 3.45.1 lacks the REGEXP operator"),
        "{}",
        rejected.reason
    );
}

#[test]
fn target_gap_is_skipped_only_under_an_exception() {
    let reference = every_capability("3.53.1");
    let mut target = every_capability("redlinedb");
    target.regexp = false;
    let partition = partition_cases(
        vec![regexp_case()],
        &reference,
        &target,
        &empty_policy(),
        "sqlite_parity",
    )
    .expect("partition");
    assert_eq!(
        partition.rejected[0].verdict_reason,
        VerdictReason::TargetUnsupported
    );
    assert!(partition.skipped.is_empty());

    let policy = policy_listing("sqlite_parity", "10547");
    let partition = partition_cases(
        vec![regexp_case()],
        &reference,
        &target,
        &policy,
        "sqlite_parity",
    )
    .expect("partition");
    assert!(partition.rejected.is_empty());
    assert_eq!(
        partition.skipped[0].policy_exception_id,
        "sqlite_parity:10547"
    );
    // The exception is per suite.
    let partition = partition_cases(vec![regexp_case()], &reference, &target, &policy, "memory")
        .expect("partition");
    assert_eq!(partition.rejected.len(), 1);
}

fn script(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    fs::write(&path, format!("#!/usr/bin/env bash\n{body}\n")).expect("script");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    wait_until_executable(&path);
    path
}

/// A child another test thread forked while this script was open for
/// writing holds a write descriptor until it execs, and exec'ing the script
/// then fails with ETXTBSY. Once one exec succeeds, none is left.
fn wait_until_executable(path: &Path) {
    for _ in 0..500 {
        match std::process::Command::new(path)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            Err(error) if error.raw_os_error() == Some(26) => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            _ => return,
        }
    }
    panic!("{} stayed busy", path.display());
}

#[test]
fn target_probe_error_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let limits = Limits::new(300, 1024).expect("limits");
    // A shell that answers --version but hangs on every probe.
    let hanging = script(
        dir.path(),
        "hanging-shell",
        "if [ \"$1\" = --version ]; then echo 'redlinedb hang'; exit 0; fi\nexec sleep 30",
    );
    let error = EngineSpec::new("redlinedb", &hanging)
        .with_limits(limits)
        .capabilities()
        .expect_err("a probe that times out is not an absent capability");
    assert!(
        format!("{error:#}").contains("ended with timeout"),
        "{error:#}"
    );
    // A binary that cannot be run at all.
    let missing = dir.path().join("missing-shell");
    assert!(
        EngineSpec::new("redlinedb", &missing)
            .with_limits(limits)
            .capabilities()
            .is_err()
    );
    // A shell that runs and refuses every probe lacks every capability.
    let refusing = script(
        dir.path(),
        "refusing-shell",
        "if [ \"$1\" = --version ]; then echo 'redlinedb refuse'; exit 0; fi\ncat >/dev/null; exit 1",
    );
    let caps = EngineSpec::new("redlinedb", &refusing)
        .with_limits(limits)
        .capabilities()
        .expect("absent capabilities");
    assert!(!caps.regexp && !caps.fts5 && !caps.math1);
}
