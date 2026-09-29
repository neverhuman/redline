//! A Strict commit stopped or failed at the WAL durability barrier: nothing
//! is published and no row lock is released early.

use super::*;

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
