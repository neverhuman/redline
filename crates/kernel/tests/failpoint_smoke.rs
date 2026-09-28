#![cfg(feature = "failpoints")]
//! Smoke tests for the Lane D failpoint scaffolding.
//!
//! These are gated on the `failpoints` feature so they are skipped under
//! default-feature workspace runs. They verify that:
//!
//! 1. `cfg("name", "panic")` actually triggers a panic at the named site,
//!    proving registry wiring through the `fail` crate.
//! 2. `fail_point!("name")` is a no-op when the failpoint is unset, proving
//!    the macro expansion stays inert in the absence of configuration.

use redlinedb_kernel::{fail_point, failpoints};

#[test]
fn cfg_panics_when_set() {
    // Each test gets its own scenario so configurations do not leak across
    // threads. We hold the scenario for the duration of the test and let the
    // unwind path drop it after we observe the panic.
    let scenario = fail::FailScenario::setup();
    failpoints::cfg("kernel::lane_d::panic_probe", "panic").expect("configure panic action");

    let result = std::panic::catch_unwind(|| {
        fail_point!("kernel::lane_d::panic_probe");
    });

    // Reset before asserting so a failed assertion does not leave a panic
    // action armed for any subsequent test on this scenario.
    failpoints::cfg("kernel::lane_d::panic_probe", "off").expect("disable panic action");
    drop(scenario);

    assert!(
        result.is_err(),
        "fail_point! with panic action must propagate a panic"
    );
}

#[test]
fn fail_point_macro_no_op_when_unset() {
    let scenario = fail::FailScenario::setup();

    // No `cfg` call has been made for this name, so the expansion must be a
    // pure no-op and execution must continue past the macro site without
    // panicking, returning, or otherwise short-circuiting the test body.
    fail_point!("kernel::lane_d::unset_probe");
    let reached = true;

    drop(scenario);
    assert!(
        reached,
        "fail_point! must be a no-op when no action is configured"
    );
}

/// Lane FP reviewer-finding: `cfg("name", "abort")` must return `Err`
/// at the kernel boundary. The `fail` crate grammar does not include
/// `abort` and the previous `cfg` wrapper forwarded the action
/// verbatim, which silently dropped the configuration and caused the
/// bench failpoint-matrix to false-pass cases that targeted `abort`.
/// Validation now rejects unknown tasks before they reach `fail::cfg`.
#[test]
fn cfg_rejects_abort() {
    let scenario = fail::FailScenario::setup();
    let err = failpoints::cfg("test::abort", "abort")
        .expect_err("abort is not a fail-crate action; cfg must reject it");
    assert!(
        err.to_lowercase().contains("abort"),
        "error message must name the unsupported token verbatim, got: {err}"
    );
    drop(scenario);
}

/// Lane E smoke: arm `engine::commit::before_publish` with `panic` and
/// confirm a kernel `Engine::commit` invocation actually triggers the
/// hook. We do not exercise recovery here because the unit-test scope
/// only needs to prove the hook is reachable from the canonical commit
/// path; the bench-level matrix proves the recovery contract.
#[test]
fn engine_commit_before_publish_hook_fires() {
    use redlinedb_kernel::engine::{Engine, EngineConfig};

    let scenario = fail::FailScenario::setup();
    failpoints::cfg("engine::commit::before_publish", "panic").expect("configure commit failpoint");

    let tmp = tempfile::tempdir().expect("tempdir");
    let engine = Engine::create(tmp.path(), EngineConfig::default()).expect("create engine");
    let mut tx = engine
        .begin(redlinedb_kernel::txn::Isolation::Snapshot)
        .expect("begin");
    engine
        .insert(&mut tx, b"value-0".to_vec())
        .expect("insert before commit");

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = engine.commit(tx);
    }));

    // Disarm before asserting so a failed assertion does not leak the
    // panic action into other tests sharing the scenario.
    failpoints::cfg("engine::commit::before_publish", "off").expect("disable commit failpoint");
    drop(scenario);

    assert!(
        result.is_err(),
        "engine::commit::before_publish must panic when armed"
    );
}

/// Lane E smoke: the same hook should also surface a non-panicking
/// maybe-committed outcome when the action requests `return(...)`.
#[test]
fn engine_commit_before_publish_returns_maybe_committed() {
    use redlinedb_kernel::engine::{
        CommitOutcome, Engine, EngineConfig, arm_commit_failure_for_thread,
    };

    let scenario = fail::FailScenario::setup();
    failpoints::cfg(
        "engine::commit::before_publish",
        "return(commit-outcome-uncertain)",
    )
    .expect("configure commit failpoint");

    let tmp = tempfile::tempdir().expect("tempdir");
    let engine = Engine::create(tmp.path(), EngineConfig::default()).expect("create engine");
    let mut tx = engine
        .begin(redlinedb_kernel::txn::Isolation::Snapshot)
        .expect("begin");
    let row = engine
        .insert(&mut tx, b"value-1".to_vec())
        .expect("insert before commit");

    arm_commit_failure_for_thread(true);
    let outcome = engine.commit(tx).expect("commit outcome");
    arm_commit_failure_for_thread(false);
    failpoints::cfg("engine::commit::before_publish", "off").expect("disable commit failpoint");
    drop(scenario);

    assert_eq!(outcome, CommitOutcome::MaybeCommitted);

    let mut observer = engine
        .begin(redlinedb_kernel::txn::Isolation::Snapshot)
        .expect("begin observer");
    assert_eq!(
        engine
            .get(&mut observer, row)
            .expect("read after maybe commit"),
        Some(b"value-1".to_vec())
    );
}

/// Strict commit must not publish a CSN if the WAL durability barrier
/// panics. The retired fast path published first and then panicked on
/// `flush_until`, which made the row visible without a durable commit.
#[test]
fn strict_commit_flush_panic_does_not_publish() {
    use redlinedb_kernel::engine::{Engine, EngineConfig};

    let scenario = fail::FailScenario::setup();
    failpoints::cfg("wal::flush_until", "panic").expect("configure flush failpoint");

    let tmp = tempfile::tempdir().expect("tempdir");
    let engine = Engine::create(tmp.path(), EngineConfig::default()).expect("create engine");
    let mut tx = engine
        .begin(redlinedb_kernel::txn::Isolation::Snapshot)
        .expect("begin");
    let row = engine
        .insert(&mut tx, b"value-flush".to_vec())
        .expect("insert before commit");

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| engine.commit(tx)));
    failpoints::cfg("wal::flush_until", "off").expect("disable flush failpoint");
    drop(scenario);

    assert!(result.is_err(), "wal::flush_until panic must abort commit");

    let mut observer = engine
        .begin(redlinedb_kernel::txn::Isolation::Snapshot)
        .expect("begin observer");
    assert_eq!(
        engine
            .get(&mut observer, row)
            .expect("read after failed commit"),
        None,
        "row must stay invisible when the durability barrier fails"
    );
}

/// Observe the live engine while a Strict writer is stopped at the barrier.
/// This catches early publication and early row-lock release independently.
#[test]
fn strict_commit_retains_visibility_and_locks_until_flush() {
    use redlinedb_kernel::engine::{CommitOutcome, Engine, EngineConfig};
    use redlinedb_kernel::txn::{Isolation, TxState};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;

    let _scenario = fail::FailScenario::setup();
    let tmp = tempfile::tempdir().unwrap();
    let engine = Engine::create(
        tmp.path(),
        EngineConfig {
            busy_timeout: Duration::from_millis(20),
            commit_durability: redlinedb_kernel::engine::CommitDurability::Strict,
            ..EngineConfig::default()
        },
    )
    .unwrap();
    let mut initial = engine.begin(Isolation::Snapshot).unwrap();
    let row = engine.insert(&mut initial, b"old".to_vec()).unwrap();
    engine.commit(initial).unwrap();
    let mut writer = engine.begin(Isolation::Snapshot).unwrap();
    engine.update(&mut writer, row, b"new".to_vec()).unwrap();
    let writer_id = writer.id();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    fail::cfg_callback("wal::flush_until", move || {
        entered_tx.send(()).unwrap();
        release_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .expect("test must release the durability barrier");
    })
    .unwrap();
    let committing = Arc::clone(&engine);
    let worker = std::thread::spawn(move || committing.commit(writer));
    entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();

    // Collect observations before asserting so even a regression releases and
    // joins the writer rather than leaving a blocked background test thread.
    let state = engine.tx_state(writer_id);
    let mut reader = engine.begin(Isolation::Snapshot).unwrap();
    let observed = engine.get(&mut reader, row);
    let mut contender = engine.begin(Isolation::Snapshot).unwrap();
    let competing_write = engine.update(&mut contender, row, b"wrong".to_vec());
    engine.rollback(contender).unwrap();
    release_tx.send(()).unwrap();
    let committed = worker.join().unwrap().unwrap();
    fail::remove("wal::flush_until");

    assert_eq!(state, TxState::InProgress);
    assert_eq!(observed.unwrap(), Some(b"old".to_vec()));
    assert_eq!(
        competing_write.unwrap_err(),
        redlinedb_kernel::Error::LockTimeout
    );
    assert!(matches!(committed, CommitOutcome::Committed(_)));
    assert_eq!(engine.get(&mut reader, row).unwrap(), Some(b"old".to_vec()));
    engine.rollback(reader).unwrap();
    let mut fresh = engine.begin(Isolation::Snapshot).unwrap();
    assert_eq!(engine.get(&mut fresh, row).unwrap(), Some(b"new".to_vec()));
    engine.update(&mut fresh, row, b"after".to_vec()).unwrap();
    engine.rollback(fresh).unwrap();
}

/// A Strict commit whose WAL records need a new segment must not be
/// acknowledged when the fsync that makes the segment's name durable fails.
/// Otherwise power loss can drop the file with the acknowledged commit in it.
///
/// Failpoints are process-global. The scenario stays held through the
/// reopen so no other test here arms a failpoint meanwhile, and
/// `return(<dir>)` limits the injected failure to this test's directory.
#[test]
fn strict_commit_fails_when_wal_dir_sync_fails() {
    use redlinedb_kernel::engine::{CommitDurability, CommitOutcome, Engine, EngineConfig};
    use redlinedb_kernel::txn::Isolation;
    use redlinedb_kernel::wal::WalConfig;

    let scenario = fail::FailScenario::setup();
    let tmp = tempfile::tempdir().expect("tempdir");
    let wal_dir = tmp.path().join("wal");
    let config = EngineConfig {
        commit_durability: CommitDurability::Strict,
        wal: WalConfig {
            segment_bytes: 64 * 1024,
            ..WalConfig::default()
        },
        ..EngineConfig::default()
    };
    let segments = || {
        std::fs::read_dir(&wal_dir)
            .expect("read wal dir")
            .filter(|entry| {
                entry
                    .as_ref()
                    .expect("wal dir entry")
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".wal")
            })
            .count()
    };
    let engine = Engine::create(tmp.path(), config.clone()).expect("create engine");
    assert_eq!(segments(), 1);
    failpoints::cfg(
        "wal::sync_dir",
        &format!("return({})", tmp.path().display()),
    )
    .expect("configure wal dir sync failpoint");

    // Commit one row at a time until the WAL has to rotate. Every commit
    // before that is acknowledged; the one that needs segment 2 is not.
    let mut acked = Vec::new();
    let mut failed = None;
    for index in 0..100_000 {
        let value = format!("row-{index:06}-{}", "v".repeat(64)).into_bytes();
        let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
        let row = match engine.insert(&mut tx, value.clone()) {
            Ok(row) => row,
            Err(err) => {
                failed = Some((None, err));
                break;
            }
        };
        match engine.commit(tx) {
            Ok(CommitOutcome::Committed(_)) => acked.push((row, value)),
            Ok(other) => panic!("unexpected commit outcome {other:?}"),
            Err(err) => {
                failed = Some((Some(row), err));
                break;
            }
        }
        assert_eq!(
            segments(),
            1,
            "commit {index} was acknowledged although its WAL rotated and \
             the directory sync failed"
        );
    }
    let (failed_row, err) = failed.expect("the WAL never rotated; lower segment_bytes");
    assert!(!acked.is_empty(), "the first commit already rotated");
    assert_eq!(segments(), 2, "the failure should come from rotation");
    let stage = if failed_row.is_some() {
        "commit"
    } else {
        "insert"
    };
    eprintln!("{stage} after {} acked rows failed: {err:?}", acked.len());

    if let Some(row) = failed_row {
        let mut observer = engine.begin(Isolation::Snapshot).expect("begin observer");
        assert_eq!(
            engine.get(&mut observer, row).expect("read failed row"),
            None,
            "a commit that failed its durability barrier must stay invisible"
        );
    }
    drop(engine);
    failpoints::cfg("wal::sync_dir", "off").expect("disable wal dir sync failpoint");

    // Every acknowledged row survives a reopen; the failed one does not.
    let engine = Engine::open(tmp.path(), config).expect("reopen engine");
    let mut reader = engine.begin(Isolation::Snapshot).expect("begin reader");
    for (row, value) in &acked {
        assert_eq!(
            engine
                .get(&mut reader, *row)
                .expect("read acked row")
                .as_ref(),
            Some(value)
        );
    }
    if let Some(row) = failed_row {
        assert_eq!(engine.get(&mut reader, row).expect("read failed row"), None);
    }
    drop(scenario);
}
