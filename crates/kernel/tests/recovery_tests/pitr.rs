//! Workplan R9, step 5: recovery to an LSN or CSN target stays at that
//! target after the next ordinary open.
//!
//! A targeted recovery leaves the WAL past the target in place, as
//! evidence. It records where the old timeline was abandoned in a durable
//! fork record at the end of the log, and every later recovery skips the
//! abandoned records.

use std::path::Path;

use redlinedb_kernel::engine::{CommitOutcome, Engine, RecoveryTarget};
use redlinedb_kernel::format::{Csn, RowId};
use redlinedb_kernel::txn::Isolation;
use redlinedb_kernel::wal::WalReader;
use tempfile::TempDir;

use super::config;
use super::generations::payload;

fn commit_row(engine: &Engine, tag: u64) -> (RowId, Csn) {
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    let row = engine.insert(&mut tx, payload(tag)).unwrap();
    match engine.commit(tx).unwrap() {
        CommitOutcome::Committed(csn) => (row, csn),
        other => panic!("commit returned {other:?}"),
    }
}

fn read(engine: &Engine, row: RowId) -> Option<Vec<u8>> {
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    engine.get(&mut tx, row).unwrap()
}

fn wal_end(root: &Path) -> redlinedb_kernel::format::Lsn {
    WalReader::new(root.join("wal"), config().wal)
        .scan_report()
        .unwrap()
        .valid_end_lsn
}

/// Rows `a` and `b` below the target, `c` past it.
struct Timeline {
    a: RowId,
    b: RowId,
    c: RowId,
}

fn three_commits(
    root: &Path,
    target_after_b: impl FnOnce(&Path, Csn) -> RecoveryTarget,
) -> (Timeline, RecoveryTarget) {
    let engine = Engine::create(root, config()).unwrap();
    let (a, _) = commit_row(&engine, 1);
    let (b, csn_b) = commit_row(&engine, 2);
    let target = target_after_b(root, csn_b);
    let (c, _) = commit_row(&engine, 3);
    drop(engine);
    (Timeline { a, b, c }, target)
}

fn assert_stays_at_target(root: &Path, rows: &Timeline, target: RecoveryTarget) {
    let restored = Engine::open_with_recovery_target(root, config(), target).unwrap();
    assert_eq!(read(&restored, rows.a), Some(payload(1)));
    assert_eq!(read(&restored, rows.b), Some(payload(2)));
    assert_eq!(
        read(&restored, rows.c),
        None,
        "the restore went past its target"
    );
    let report = restored.last_recovery_report().expect("a recovery report");
    assert_eq!(report.target, target);
    let fork = report.timeline_fork.expect("the restore recorded a fork");
    assert!(fork.fork_lsn < fork.record_lsn, "{fork:?}");
    assert!(report.abandoned_wal.is_empty(), "{report:?}");
    drop(restored);

    // The next ordinary open must not replay the abandoned commit.
    let reopened = Engine::open(root, config()).unwrap();
    let report = reopened.last_recovery_report().expect("a recovery report");
    assert_eq!(report.abandoned_wal, vec![(fork.fork_lsn, fork.record_lsn)]);
    assert_eq!(report.timeline_fork, None);
    assert_eq!(read(&reopened, rows.a), Some(payload(1)));
    assert_eq!(read(&reopened, rows.b), Some(payload(2)));
    assert_eq!(
        read(&reopened, rows.c),
        None,
        "a normal open after the restore replayed past the target"
    );
    let (d, _) = commit_row(&reopened, 4);
    assert_ne!(d, rows.c, "the new timeline reused an abandoned row id");
    drop(reopened);

    // New work on the restored timeline survives, and the abandoned commit
    // stays abandoned.
    let again = Engine::open(root, config()).unwrap();
    assert_eq!(read(&again, rows.a), Some(payload(1)));
    assert_eq!(read(&again, rows.b), Some(payload(2)));
    assert_eq!(read(&again, rows.c), None);
    assert_eq!(read(&again, d), Some(payload(4)));
}

#[test]
fn recovery_to_an_lsn_stays_there_after_reopen() {
    let temp = TempDir::new().unwrap();
    let (rows, target) = three_commits(temp.path(), |root, _| RecoveryTarget::Lsn(wal_end(root)));
    assert_stays_at_target(temp.path(), &rows, target);
}

#[test]
fn recovery_to_a_csn_stays_there_after_reopen() {
    let temp = TempDir::new().unwrap();
    let (rows, target) = three_commits(temp.path(), |_, csn_b| RecoveryTarget::Csn(csn_b));
    assert_stays_at_target(temp.path(), &rows, target);
}

#[test]
fn a_second_restore_to_an_earlier_lsn_stays_there_too() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let engine = Engine::create(root, config()).unwrap();
    let (a, _) = commit_row(&engine, 1);
    let first_target = RecoveryTarget::Lsn(wal_end(root));
    let (b, _) = commit_row(&engine, 2);
    let second_target = RecoveryTarget::Lsn(wal_end(root));
    let (c, _) = commit_row(&engine, 3);
    drop(engine);

    // Restore to after b, add d on that timeline, then restore to after a.
    let restored = Engine::open_with_recovery_target(root, config(), second_target).unwrap();
    assert_eq!(read(&restored, c), None);
    drop(restored);
    let reopened = Engine::open(root, config()).unwrap();
    let (d, _) = commit_row(&reopened, 4);
    drop(reopened);
    let restored = Engine::open_with_recovery_target(root, config(), first_target).unwrap();
    assert_eq!(read(&restored, a), Some(payload(1)));
    assert_eq!(read(&restored, b), None);
    assert_eq!(read(&restored, d), None);
    drop(restored);

    let reopened = Engine::open(root, config()).unwrap();
    assert_eq!(read(&reopened, a), Some(payload(1)));
    assert_eq!(read(&reopened, b), None);
    assert_eq!(read(&reopened, c), None);
    assert_eq!(read(&reopened, d), None);
}

#[test]
fn a_target_past_the_end_of_the_wal_records_no_fork() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let engine = Engine::create(root, config()).unwrap();
    let (a, csn) = commit_row(&engine, 1);
    drop(engine);
    let end = wal_end(root);

    for target in [RecoveryTarget::Lsn(end), RecoveryTarget::Csn(csn)] {
        let restored = Engine::open_with_recovery_target(root, config(), target).unwrap();
        assert_eq!(read(&restored, a), Some(payload(1)));
        assert_eq!(restored.last_recovery_report().unwrap().timeline_fork, None);
        drop(restored);
        assert_eq!(
            wal_end(root),
            end,
            "a restore with nothing past it wrote to the WAL"
        );
    }
}
