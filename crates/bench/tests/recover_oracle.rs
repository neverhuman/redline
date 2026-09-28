//! The recovery oracle and the recover gates must fail closed.
//!
//! The unit cases start from the image a correct recovery produces and
//! apply one defect each; every defect must disqualify the run. The CLI
//! cases drive the real `redlinedb-bench` binary: a child that never starts
//! must fail the command, and a healthy crash-and-recover must pass it.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use redlinedb_bench::config::{DurabilityKind, EngineKind, RecoveryScenarioKind};
use redlinedb_bench::recover::harness::CaseOutcome;
use redlinedb_bench::recover::oracle::{
    self, AckLedger, KV_INDEX, KV_TABLE, PROGRESS_TABLE, RecoveredState, Workload, evaluate,
};
use redlinedb_bench::recover::{RecoveryMatrixReport, RecoveryMatrixRun};

const ROWS: usize = 64;

fn ledger(workload: Workload, acked: std::ops::Range<u64>) -> AckLedger {
    let mut ledger = AckLedger::new(workload, ROWS, true);
    for key in acked {
        ledger.ack(key);
    }
    ledger
}

#[test]
fn exact_recovery_qualifies_with_or_without_the_in_flight_key() {
    let expected = ledger(Workload::RecoverWal, 0..5);
    let verdict = evaluate(
        &expected,
        &RecoveredState::consistent(Workload::RecoverWal, ROWS, 0..5),
    );
    assert!(verdict.qualified, "{}", verdict.summary());
    assert_eq!(verdict.recovered_acked, 5);
    assert_eq!(verdict.in_flight_committed, None);

    let verdict = evaluate(
        &expected,
        &RecoveredState::consistent(Workload::RecoverWal, ROWS, 0..6),
    );
    assert!(verdict.qualified, "{}", verdict.summary());
    assert_eq!(verdict.in_flight_committed, Some(5));

    // Two keys past the last ack is one too many.
    let verdict = evaluate(
        &expected,
        &RecoveredState::consistent(Workload::RecoverWal, ROWS, 0..7),
    );
    assert!(!verdict.qualified);
    assert!(
        verdict
            .unexpected_effects
            .iter()
            .any(|line| line.contains("key 6")),
        "{:?}",
        verdict.unexpected_effects
    );
}

#[test]
fn substituted_id_fails() {
    // Same row count as the ledger (5), but key 4 came back as key 9.
    let expected = ledger(Workload::RecoverWal, 0..5);
    let observed = RecoveredState::consistent(Workload::RecoverWal, ROWS, [0, 1, 2, 3, 9]);
    let verdict = evaluate(&expected, &observed);
    assert!(!verdict.qualified);
    assert_eq!(verdict.lost_ack_ids, vec![4]);
    assert!(
        verdict
            .unexpected_effects
            .iter()
            .any(|line| line.contains("key 9")),
        "{:?}",
        verdict.unexpected_effects
    );
}

#[test]
fn changed_value_fails() {
    // Key 2 is present in both tables, but its kv row holds the value the
    // checkpoint workload would write, not the acknowledged one.
    let expected = ledger(Workload::RecoverWal, 0..5);
    let mut observed = RecoveredState::consistent(Workload::RecoverWal, ROWS, 0..5);
    let other = oracle::expected_rows(Workload::RecoverCheckpoint, 2, ROWS)[KV_TABLE].clone();
    observed.tables.get_mut(KV_TABLE).unwrap().insert(2, other);
    let verdict = evaluate(&expected, &observed);
    assert!(!verdict.qualified);
    assert_eq!(verdict.lost_ack_ids, vec![2]);
    assert!(
        verdict
            .unexpected_effects
            .iter()
            .any(|line| line.contains("key 2") && line.contains("kv row differs")),
        "{:?}",
        verdict.unexpected_effects
    );
}

#[test]
fn missing_index_entry_fails() {
    let expected = ledger(Workload::RecoverWal, 0..5);
    let mut observed = RecoveredState::consistent(Workload::RecoverWal, ROWS, 0..5);
    let check = observed
        .index_checks
        .iter_mut()
        .find(|check| check.index == KV_INDEX)
        .unwrap();
    // Key 2 has tenant 2; the heap still has it, the index lost it.
    assert!(check.via_index.get_mut("2").unwrap().remove(&2));
    let verdict = evaluate(&expected, &observed);
    assert!(!verdict.qualified);
    assert!(verdict.lost_ack_ids.is_empty());
    assert!(
        verdict
            .integrity_errors
            .iter()
            .any(|line| line.contains(KV_INDEX) && line.contains("missing from index [2]")),
        "{:?}",
        verdict.integrity_errors
    );

    // An index probe that silently fell back to a scan proves nothing.
    let mut observed = RecoveredState::consistent(Workload::RecoverWal, ROWS, 0..5);
    observed.index_checks[0].index_path_used = false;
    let verdict = evaluate(&expected, &observed);
    assert!(!verdict.qualified);
    assert!(verdict.integrity_errors[0].contains("did not read through the index"));

    // So does an index nobody checked.
    let mut observed = RecoveredState::consistent(Workload::RecoverWal, ROWS, 0..5);
    observed.index_checks.clear();
    assert!(!evaluate(&expected, &observed).qualified);
}

#[test]
fn an_index_check_that_covers_other_rows_than_the_table_fails() {
    // The scan and the probes agree with each other, but on nothing the
    // table holds: pre-seeded empty probes must not pass for a check.
    let expected = ledger(Workload::RecoverWal, 0..5);
    let mut observed = RecoveredState::consistent(Workload::RecoverWal, ROWS, 0..5);
    let check = &mut observed.index_checks[0];
    for keys in check
        .via_index
        .values_mut()
        .chain(check.via_scan.values_mut())
    {
        keys.clear();
    }
    let verdict = evaluate(&expected, &observed);
    assert!(!verdict.qualified, "an empty index check qualified");
    assert!(
        verdict
            .integrity_errors
            .iter()
            .any(|line| line.contains(KV_INDEX) && line.contains("full scan")),
        "{:?}",
        verdict.integrity_errors
    );
}

#[test]
fn half_transaction_fails() {
    // Acked key 4: crash_progress row survived, kv row did not.
    let expected = ledger(Workload::RecoverWal, 0..5);
    let mut observed = RecoveredState::consistent(Workload::RecoverWal, ROWS, 0..5);
    observed.tables.get_mut(KV_TABLE).unwrap().remove(&4);
    for check in &mut observed.index_checks {
        for keys in check
            .via_index
            .values_mut()
            .chain(check.via_scan.values_mut())
        {
            keys.remove(&4);
        }
    }
    let verdict = evaluate(&expected, &observed);
    assert!(!verdict.qualified);
    assert_eq!(verdict.partial_transactions, vec![4]);
    assert_eq!(verdict.lost_ack_ids, vec![4]);

    // The in-flight key may be absent or complete, never half there.
    let mut observed = RecoveredState::consistent(Workload::RecoverWal, ROWS, 0..5);
    let progress = oracle::expected_rows(Workload::RecoverWal, 5, ROWS)[PROGRESS_TABLE].clone();
    observed
        .tables
        .get_mut(PROGRESS_TABLE)
        .unwrap()
        .insert(5, progress);
    let verdict = evaluate(&expected, &observed);
    assert!(!verdict.qualified);
    assert_eq!(verdict.partial_transactions, vec![5]);
    assert!(verdict.lost_ack_ids.is_empty());
}

#[test]
fn zero_acks_without_ready_fails() {
    let expected = ledger(Workload::RecoverWal, 0..0);
    let verdict = evaluate(&expected, &RecoveredState::default());
    assert!(!verdict.qualified);
    assert!(!verdict.child_started);
    assert!(verdict.summary().contains("READY"), "{}", verdict.summary());

    // Even a clean-looking empty database does not help without READY.
    let mut observed = RecoveredState::consistent(Workload::RecoverWal, ROWS, 0..0);
    observed.child_started = false;
    assert!(!evaluate(&expected, &observed).qualified);
}

#[test]
fn missing_fault_evidence_fails_when_a_fault_is_expected() {
    let mut observed = RecoveredState::consistent(Workload::RecoverWal, ROWS, 0..5);
    observed.fault_observed = false;
    let verdict = evaluate(&ledger(Workload::RecoverWal, 0..5), &observed);
    assert!(!verdict.qualified);
    assert!(
        verdict.summary().contains("no fault"),
        "{}",
        verdict.summary()
    );

    let mut no_fault_expected = ledger(Workload::RecoverWal, 0..5);
    no_fault_expected.expect_fault = false;
    assert!(evaluate(&no_fault_expected, &observed).qualified);
}

#[test]
fn integrity_check_and_harness_errors_fail() {
    let expected = ledger(Workload::RecoverWal, 0..5);
    let mut observed = RecoveredState::consistent(Workload::RecoverWal, ROWS, 0..5);
    observed.integrity_check = vec!["row 3 missing from index kv_tenant_idx".to_owned()];
    assert!(!evaluate(&expected, &observed).qualified);

    let mut observed = RecoveredState::consistent(Workload::RecoverWal, ROWS, 0..5);
    observed
        .harness_errors
        .push("table kv differs between recovery passes".to_owned());
    assert!(!evaluate(&expected, &observed).qualified);
}

#[test]
fn catalog_schema_must_match_the_committed_keys() {
    // Keys 0..6: odd keys 1, 3, 5 keep scratch_1, scratch_3, scratch_5.
    let expected = ledger(Workload::RecoverCatalog, 0..6);
    let healthy = RecoveredState::consistent(Workload::RecoverCatalog, ROWS, 0..6);
    let want: BTreeSet<String> = [
        "crash_progress",
        "kv",
        "kv_tenant_idx",
        "scratch_1",
        "scratch_1_note_idx",
        "scratch_3",
        "scratch_3_note_idx",
        "scratch_5",
        "scratch_5_note_idx",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(healthy.schema, want);
    let verdict = evaluate(&expected, &healthy);
    assert!(verdict.qualified, "{}", verdict.summary());

    // An even key's table must have been dropped in its own transaction.
    let mut observed = healthy.clone();
    observed.schema.insert("scratch_0".to_owned());
    let verdict = evaluate(&expected, &observed);
    assert!(!verdict.qualified);
    assert!(
        verdict
            .unexpected_effects
            .iter()
            .any(|l| l.contains("scratch_0"))
    );

    // A committed odd key's table must exist.
    let mut observed = healthy.clone();
    observed.schema.remove("scratch_3");
    observed.schema.remove("scratch_3_note_idx");
    observed.tables.remove("scratch_3");
    observed
        .index_checks
        .retain(|check| check.table != "scratch_3");
    let verdict = evaluate(&expected, &observed);
    assert!(!verdict.qualified);
    assert_eq!(verdict.partial_transactions, vec![3]);
    assert!(
        verdict
            .integrity_errors
            .iter()
            .any(|l| l.contains("scratch_3"))
    );
}

#[test]
fn ledger_parse_fails_closed() {
    let workload = Workload::RecoverWal;
    let line = |key: u64| oracle::ack_line(key, &oracle::expected_txn_digest(workload, key, ROWS));

    let text = format!("{}{}", line(0), line(1));
    let parsed = AckLedger::parse(workload, ROWS, true, &text).unwrap();
    assert_eq!(parsed.acked.keys().copied().collect::<Vec<_>>(), vec![0, 1]);
    assert_eq!(parsed.in_flight_key(), Some(2));

    // An unfinished last line is an ack that never completed.
    let torn = format!("{}{}", line(0), &line(1)[..5]);
    let parsed = AckLedger::parse(workload, ROWS, true, &torn).unwrap();
    assert_eq!(parsed.acked.len(), 1);
    assert!(parsed.torn_tail.is_some());

    assert!(
        AckLedger::parse(workload, ROWS, true, "")
            .unwrap()
            .acked
            .is_empty()
    );
    // Out of order, a bare key without digest, and a digest of other
    // contents are all rejected.
    assert!(AckLedger::parse(workload, ROWS, true, &line(1)).is_err());
    assert!(AckLedger::parse(workload, ROWS, true, "0\n").is_err());
    let wrong = oracle::ack_line(
        0,
        &oracle::expected_txn_digest(Workload::RecoverCheckpoint, 0, ROWS),
    );
    assert!(AckLedger::parse(workload, ROWS, true, &wrong).is_err());
}

fn synthetic_run(passed: bool) -> RecoveryMatrixRun {
    let verdict = evaluate(
        &ledger(Workload::RecoverWal, 0..3),
        &RecoveredState::consistent(Workload::RecoverWal, ROWS, 0..3),
    );
    RecoveryMatrixRun {
        case: "synthetic".to_owned(),
        engine: EngineKind::Redline,
        durability: DurabilityKind::Strict,
        scenario: RecoveryScenarioKind::Wal,
        kill_after_ms: 1,
        passed,
        outcome: CaseOutcome {
            acknowledged: 3,
            recovered: 3,
            child_status: "signal(9)".to_owned(),
            kill_result: "sent".to_owned(),
            verdict,
            evidence: None,
        },
    }
}

#[test]
fn synthetic_failed_run_fails_the_gate() {
    let report =
        RecoveryMatrixReport::from_runs(7, None, vec![synthetic_run(true), synthetic_run(false)]);
    assert!(!report.passed);
    assert_eq!(report.failed_cases, 1);
    let err = report
        .ensure_passed()
        .expect_err("a failed run must fail the gate");
    assert!(
        err.to_string().contains("1/2 runs did not qualify"),
        "{err}"
    );

    let empty = RecoveryMatrixReport::from_runs(7, None, Vec::new());
    assert!(!empty.passed);
    assert!(empty.ensure_passed().is_err(), "no runs is not a pass");

    let mut forged = RecoveryMatrixReport::from_runs(7, None, vec![synthetic_run(true)]);
    forged.ensure_passed().expect("one qualified run passes");
    forged.passed = false;
    assert!(
        forged.ensure_passed().is_err(),
        "passed=false must fail the gate"
    );
}

fn write_matrix(dir: &Path, rows: usize, kill_ms: u64) -> std::path::PathBuf {
    let path = dir.join("matrix.toml");
    std::fs::write(
        &path,
        format!(
            "durabilities = [\"strict\"]\n\n[[cases]]\nname = \"wal\"\nscenario = \"wal\"\nrows = {rows}\nkill_windows_ms = [{kill_ms}]\ncheckpoint_every_rows = 64\n"
        ),
    )
    .unwrap();
    path
}

fn read_report(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).expect("report written")).expect("report is JSON")
}

#[test]
fn recover_matrix_exits_nonzero_when_child_fails_startup() {
    let tmp = tempfile::tempdir().unwrap();
    let config = write_matrix(tmp.path(), 64, 50);
    let out = tmp.path().join("report.json");
    let output = Command::new(env!("CARGO_BIN_EXE_redlinedb-bench"))
        .args(["recover-matrix", "--engine", "redline", "--config"])
        .arg(&config)
        .args(["--child-exe", "/bin/false", "--out"])
        .arg(&out)
        // Kept failure evidence lands inside this test's tempdir.
        .env("TMPDIR", tmp.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "must exit non-zero; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("recover-matrix gate failed"),
        "stderr:\n{stderr}"
    );

    let report = read_report(&out);
    assert_eq!(report["passed"], false);
    let run = &report["runs"][0];
    assert_eq!(run["passed"], false);
    assert_eq!(run["verdict"]["child_started"], false);
    let evidence = run["evidence"]["dir"]
        .as_str()
        .expect("failure keeps evidence");
    assert!(Path::new(evidence).starts_with(tmp.path()), "{evidence}");
    assert!(Path::new(evidence).exists());
}

#[test]
fn recover_matrix_healthy_single_case_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    // Far more rows than a strict child can commit in 500 ms, so the kill
    // always lands on a live child.
    let config = write_matrix(tmp.path(), 1_000_000, 500);
    let out = tmp.path().join("report.json");
    let output = Command::new(env!("CARGO_BIN_EXE_redlinedb-bench"))
        .args(["recover-matrix", "--engine", "both", "--config"])
        .arg(&config)
        .arg("--out")
        .arg(&out)
        .env("TMPDIR", tmp.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "healthy matrix must exit 0; stderr:\n{stderr}"
    );

    let report = read_report(&out);
    assert_eq!(report["passed"], true);
    let runs = report["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2, "one run per engine");
    for run in runs {
        assert_eq!(run["passed"], true, "{run:#}");
        assert_eq!(run["verdict"]["child_started"], true, "{run:#}");
        assert_eq!(run["verdict"]["fault_observed"], true, "{run:#}");
        assert!(run["acknowledged"].as_u64().unwrap() > 0, "{run:#}");
        assert!(run.get("evidence").is_none(), "{run:#}");
    }
}
