//! `PRAGMA user_version` fsyncs its sidecar under the live commit
//! durability, which `PRAGMA synchronous` changes, like the catalog does.
//!
//! The sync counters are per thread, so each test counts only its own saves.

use std::cell::Cell;

use redlinedb_kernel::engine::CommitDurability;

use crate::{Database, DbOptions};

thread_local! {
    static FILE_SYNCS: Cell<u64> = const { Cell::new(0) };
    static DIR_SYNCS: Cell<u64> = const { Cell::new(0) };
}

pub(super) fn note_sync(dir: bool) {
    let counter = if dir { &DIR_SYNCS } else { &FILE_SYNCS };
    counter.with(|count| count.set(count.get() + 1));
}

/// (sidecar fsyncs, directory fsyncs) made by this thread.
fn syncs() -> (u64, u64) {
    (FILE_SYNCS.with(Cell::get), DIR_SYNCS.with(Cell::get))
}

fn user_version_syncs(opened: CommitDurability, synchronous: &str) -> (u64, u64) {
    let dir = tempfile::tempdir().unwrap();
    let mut opts = DbOptions::default();
    opts.engine.commit_durability = opened;
    let db = Database::create(dir.path().join("db"), opts).unwrap();
    let conn = db.connect();
    conn.execute(&format!("PRAGMA synchronous = {synchronous}"))
        .unwrap();
    let before = syncs();
    conn.execute("PRAGMA user_version = 7").unwrap();
    let after = syncs();
    assert_eq!(db.user_version(), 7);
    (after.0 - before.0, after.1 - before.1)
}

#[test]
fn user_version_fsync_follows_a_later_synchronous_pragma() {
    for (opened, synchronous, expected) in [
        (CommitDurability::Normal, "FULL", (1, 1)),
        (CommitDurability::Strict, "FULL", (1, 1)),
        (CommitDurability::Strict, "NORMAL", (0, 0)),
        (CommitDurability::Normal, "NORMAL", (0, 0)),
    ] {
        assert_eq!(
            user_version_syncs(opened, synchronous),
            expected,
            "opened {opened:?}, PRAGMA synchronous = {synchronous}"
        );
    }
}
