#![cfg(feature = "failpoints")]
//! What a commit reports when the WAL fails around it, and what the next
//! open finds (workplan R7).
//!
//! A commit that fails before its record is queued is certainly aborted: it
//! returns a plain error and never comes back. Once the record is queued, a
//! WAL failure leaves the outcome unknown: the commit returns
//! `CommitOutcomeUnknown`, is invisible in this process, and the next open
//! finds it committed exactly when its record reached the file.
//!
//! Failpoints are process-global and the WAL writer runs on its own thread,
//! so a thread-local switch cannot confine a fault. Each test holds a
//! `fail::FailScenario`, which serializes them, and arms its fault with
//! `return(<its temp dir>)`, which fails only that directory's WAL.
//!
//! Run: `cargo test -p redlinedb-kernel --features failpoints --test
//! commit_outcome -- --test-threads=1`

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use redlinedb_kernel::Error;
use redlinedb_kernel::engine::{CommitDurability, CommitOutcome, Engine, EngineConfig};
use redlinedb_kernel::failpoints;
use redlinedb_kernel::format::RowId;
use redlinedb_kernel::txn::Isolation;
use redlinedb_kernel::wal::{WalConfig, WalFailureStage};

/// Long enough for any healthy commit, short enough to fail a hung test.
const COMMIT_DEADLINE: Duration = Duration::from_secs(20);

fn config(durability: CommitDurability) -> EngineConfig {
    EngineConfig {
        commit_durability: durability,
        wal: WalConfig {
            group_commit_delay_us: 0,
            ..WalConfig::default()
        },
        ..EngineConfig::default()
    }
}

fn arm(name: &str, dir: &Path) {
    failpoints::cfg(name, &format!("return({})", dir.display())).expect("arm failpoint");
}

fn disarm(name: &str) {
    failpoints::cfg(name, "off").expect("disarm failpoint");
}

fn read(engine: &Engine, row: RowId) -> Option<Vec<u8>> {
    let mut tx = engine.begin(Isolation::Snapshot).expect("begin reader");
    engine.get(&mut tx, row).expect("read row")
}

/// Insert `value` and commit it on another thread, failing the test if the
/// commit does not return in time.
fn insert_and_commit(engine: &Arc<Engine>, value: &[u8]) -> (RowId, Result<CommitOutcome, Error>) {
    let (done, wait) = mpsc::channel();
    let engine = Arc::clone(engine);
    let value = value.to_vec();
    thread::spawn(move || {
        let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
        let row = engine.insert(&mut tx, value).expect("insert");
        let _ = done.send((row, engine.commit(tx)));
    });
    wait.recv_timeout(COMMIT_DEADLINE)
        .expect("commit did not return: is the published CSN frontier stuck?")
}

fn expect_unknown(result: Result<CommitOutcome, Error>, stage: WalFailureStage) {
    let err = match result {
        Err(err) => err,
        Ok(outcome) => {
            panic!("expected an unknown outcome after a {stage} failure, got {outcome:?}")
        }
    };
    assert!(
        err.to_string().contains("wal writer failed"),
        "the message should name the WAL failure: {err}"
    );
    match err {
        Error::CommitOutcomeUnknown { cause, .. } => match *cause {
            Error::WalWriterFailed { stage: seen, .. } => assert_eq!(seen, stage),
            other => panic!("unknown outcome carries cause {other:?}"),
        },
        other => panic!("expected CommitOutcomeUnknown after a {stage} failure, got {other:?}"),
    }
}

/// The commit fails after its CSN is reserved and before its record is
/// queued. That is a certain abort, so it is a plain error. The reserved CSN
/// must not hold back the published frontier: a later commit is visible to a
/// new snapshot, before and after a reopen.
#[test]
fn commit_failing_before_enqueue_is_aborted_and_leaves_no_csn_hole() {
    let scenario = fail::FailScenario::setup();
    let dir = tempfile::tempdir().expect("tempdir");
    let config = config(CommitDurability::Strict);
    let engine = Engine::create(dir.path(), config.clone()).expect("create engine");

    arm("wal::append_commit_error", dir.path());
    let (aborted_row, aborted) = insert_and_commit(&engine, b"aborted");
    disarm("wal::append_commit_error");
    match aborted {
        Err(Error::Io(err)) => assert!(
            err.to_string().contains("wal::append_commit_error"),
            "unexpected io error {err}"
        ),
        other => panic!("a commit that never reached the WAL must fail plainly, got {other:?}"),
    }
    assert_eq!(read(&engine, aborted_row), None);

    let (later_row, later) = insert_and_commit(&engine, b"later");
    assert!(
        matches!(later, Ok(CommitOutcome::Committed(_))),
        "later commit: {later:?}"
    );
    assert_eq!(
        read(&engine, later_row).as_deref(),
        Some(&b"later"[..]),
        "a commit after the aborted one returned, but a new snapshot misses it"
    );
    drop(engine);

    let engine = Engine::open(dir.path(), config).expect("reopen engine");
    assert_eq!(
        read(&engine, aborted_row),
        None,
        "a certain abort came back"
    );
    assert_eq!(
        read(&engine, later_row).as_deref(),
        Some(&b"later"[..]),
        "the later commit is missing after reopen"
    );
    drop(scenario);
}

/// The commit record is written, then its fsync fails. The commit may be on
/// disk, so it reports an unknown outcome, not an abort. This process does
/// not show it, and later writes fail. The next open finds the record, which
/// is still in the file, and the row.
#[test]
fn fsync_failure_after_write_is_unknown_and_recovers_committed() {
    let scenario = fail::FailScenario::setup();
    let dir = tempfile::tempdir().expect("tempdir");
    let config = config(CommitDurability::Strict);
    let engine = Engine::create(dir.path(), config.clone()).expect("create engine");
    let (kept_row, kept) = insert_and_commit(&engine, b"before");
    assert!(matches!(kept, Ok(CommitOutcome::Committed(_))));

    arm("wal::flush_error", dir.path());
    let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
    let tx_id = tx.id();
    let row = engine
        .insert(&mut tx, b"uncertain".to_vec())
        .expect("insert");
    let result = engine.commit(tx);
    if let Err(Error::CommitOutcomeUnknown {
        tx_id: seen,
        end_lsn,
        ..
    }) = &result
    {
        assert_eq!(*seen, tx_id, "the error names the transaction");
        assert!(end_lsn.0 > 0, "the error names the commit record's end");
    }
    expect_unknown(result, WalFailureStage::Flush);
    assert_eq!(read(&engine, row), None, "an unknown commit is not shown");
    assert_eq!(read(&engine, kept_row).as_deref(), Some(&b"before"[..]));

    // The writer stopped. A later write fails and names the failure.
    let mut tx = engine
        .begin(Isolation::Snapshot)
        .expect("begin after failure");
    match engine.insert(&mut tx, b"after".to_vec()) {
        Err(Error::WalWriterFailed { stage, .. }) => assert_eq!(stage, WalFailureStage::Flush),
        other => panic!("a write after the WAL failed must fail, got {other:?}"),
    }
    drop(tx);
    drop(engine);
    disarm("wal::flush_error");

    let engine = Engine::open(dir.path(), config).expect("reopen engine");
    assert_eq!(read(&engine, kept_row).as_deref(), Some(&b"before"[..]));
    assert_eq!(
        read(&engine, row).as_deref(),
        Some(&b"uncertain"[..]),
        "the written commit record should recover"
    );
    drop(scenario);
}

/// Writing the commit record fails. Its outcome is reported unknown, and
/// since nothing reached the file, the next open does not find it. Normal
/// waits for the write and Strict for the fsync; both see the write fail.
///
/// The row's own record goes out first, so the fault hits the batch that
/// holds the commit record. Had it hit the row's record, the commit would
/// have failed before it was queued, a certain abort.
#[test]
fn write_failure_is_unknown_and_recovers_aborted() {
    for durability in [CommitDurability::Strict, CommitDurability::Normal] {
        let scenario = fail::FailScenario::setup();
        let dir = tempfile::tempdir().expect("tempdir");
        let config = config(durability);
        let engine = Engine::create(dir.path(), config.clone()).expect("create engine");
        let (kept_row, kept) = insert_and_commit(&engine, b"before");
        assert!(matches!(kept, Ok(CommitOutcome::Committed(_))));

        let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
        let row = engine.insert(&mut tx, b"lost".to_vec()).expect("insert");
        engine.checkpoint().expect("write the row's record out");
        arm("wal::write_error", dir.path());
        let (done, wait) = mpsc::channel();
        let committer = Arc::clone(&engine);
        thread::spawn(move || {
            let _ = done.send(committer.commit(tx));
        });
        let result = wait
            .recv_timeout(COMMIT_DEADLINE)
            .expect("the commit did not return after its write failed");
        expect_unknown(result, WalFailureStage::Write);
        assert_eq!(
            read(&engine, row),
            None,
            "{durability:?}: unknown is not shown"
        );
        drop(engine);
        disarm("wal::write_error");

        let engine = Engine::open(dir.path(), config).expect("reopen engine");
        assert_eq!(read(&engine, kept_row).as_deref(), Some(&b"before"[..]));
        assert_eq!(
            read(&engine, row),
            None,
            "{durability:?}: a record never written must not recover"
        );
        drop(scenario);
    }
}

/// Several commits wait on one failing fsync. Every one of them returns,
/// none reports success, and none that reported a certain abort recovers.
#[test]
fn every_waiting_commit_wakes_on_failure_without_false_success() {
    let scenario = fail::FailScenario::setup();
    let dir = tempfile::tempdir().expect("tempdir");
    let config = config(CommitDurability::Strict);
    let engine = Engine::create(dir.path(), config.clone()).expect("create engine");

    arm("wal::flush_error", dir.path());
    let (done, wait) = mpsc::channel();
    let writers = 8;
    for writer in 0..writers {
        let engine = Arc::clone(&engine);
        let done = done.clone();
        thread::spawn(move || {
            let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
            let result = match engine.insert(&mut tx, format!("writer-{writer}").into_bytes()) {
                Ok(row) => (Some(row), engine.commit(tx)),
                Err(err) => (None, Err(err)),
            };
            let _ = done.send(result);
        });
    }
    drop(done);
    let mut certain_aborts = Vec::new();
    let mut unknown = 0;
    for _ in 0..writers {
        let (row, result) = wait
            .recv_timeout(COMMIT_DEADLINE)
            .expect("a commit waiting on the failed WAL never returned");
        match result {
            Ok(outcome) => panic!("commit succeeded after its fsync failed: {outcome:?}"),
            Err(Error::CommitOutcomeUnknown { .. }) => unknown += 1,
            Err(Error::WalWriterFailed { .. }) => certain_aborts.extend(row),
            Err(other) => panic!("unexpected error {other:?}"),
        }
    }
    assert!(
        unknown > 0,
        "the first commit's fsync failed, so it is unknown"
    );
    drop(engine);
    disarm("wal::flush_error");

    let engine = Engine::open(dir.path(), config).expect("reopen engine");
    for row in certain_aborts {
        assert_eq!(
            read(&engine, row),
            None,
            "a certainly aborted commit came back"
        );
    }
    drop(scenario);
}
