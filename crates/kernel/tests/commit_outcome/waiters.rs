//! Many commits waiting on one failing fsync: each wakes, none reports
//! success, and no certain abort comes back after reopen.

use super::*;

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
