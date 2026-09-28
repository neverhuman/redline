//! S9-05 kernel support: a transaction can begin in an earlier
//! transaction's snapshot, and a BEGIN IMMEDIATE reservation can move to it
//! without ever being free.

use std::sync::Arc;
use std::time::Duration;

use redlinedb_kernel::engine::{Engine, EngineConfig};
use redlinedb_kernel::format::RelId;
use redlinedb_kernel::txn::Isolation;
use redlinedb_kernel::wal::WalConfig;
use tempfile::TempDir;

fn engine() -> (TempDir, Arc<Engine>) {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(
        temp.path(),
        EngineConfig {
            rel_id: RelId(1),
            wal: WalConfig {
                segment_bytes: 65536,
                ..WalConfig::default()
            },
            commit_durability: redlinedb_kernel::engine::CommitDurability::Strict,
            lock_shards: 32,
            busy_timeout: Duration::from_millis(100),
            heap_lanes: 16,
            page_size: redlinedb_kernel::format::DEFAULT_PAGE_SIZE,
            buffer_pool_pages: 256,
            data_file_name: "data.redline".to_owned(),
        },
    )
    .unwrap();
    (temp, engine)
}

#[test]
fn a_transaction_begun_with_an_older_snapshot_reads_through_it() {
    let (_temp, engine) = engine();
    let mut seed = engine.begin(Isolation::Snapshot).unwrap();
    let row = engine.insert(&mut seed, b"old".to_vec()).unwrap();
    engine.commit(seed).unwrap();

    let mut original = engine.begin(Isolation::Snapshot).unwrap();
    // A concurrent writer commits after `original` took its snapshot.
    let mut writer = engine.begin(Isolation::Snapshot).unwrap();
    engine.update(&mut writer, row, b"new".to_vec()).unwrap();
    let added = engine.insert(&mut writer, b"added".to_vec()).unwrap();
    engine.commit(writer).unwrap();

    let snapshot = original.snapshot().clone();
    let mut replay = engine
        .begin_with_snapshot(Isolation::Snapshot, snapshot)
        .unwrap();
    assert_ne!(replay.id(), original.id());
    engine.rollback(original).unwrap();
    assert_eq!(engine.get(&mut replay, row).unwrap(), Some(b"old".to_vec()));
    assert_eq!(engine.get(&mut replay, added).unwrap(), None);
    // Its own writes are visible to it.
    let mine = engine.insert(&mut replay, b"mine".to_vec()).unwrap();
    assert_eq!(
        engine.get(&mut replay, mine).unwrap(),
        Some(b"mine".to_vec())
    );
    // Writing a row changed after the snapshot is a conflict, as it would
    // have been for the original transaction.
    assert!(engine.update(&mut replay, row, b"lost".to_vec()).is_err());
    engine.rollback(replay).unwrap();

    let mut fresh = engine.begin(Isolation::Snapshot).unwrap();
    assert_eq!(engine.get(&mut fresh, row).unwrap(), Some(b"new".to_vec()));
    assert_eq!(engine.get(&mut fresh, mine).unwrap(), None);
}

#[test]
fn a_begin_reservation_moves_without_ever_being_free() {
    let (_temp, engine) = engine();
    let mut holder = engine.begin(Isolation::Snapshot).unwrap();
    engine.reserve_begin_lock(&mut holder).unwrap();
    let mut heir = engine
        .begin_with_snapshot(Isolation::Snapshot, holder.snapshot().clone())
        .unwrap();
    assert!(engine.transfer_begin_lock(&mut holder, &mut heir));
    // Rolling the old holder back does not free the reservation.
    engine.rollback(holder).unwrap();
    let mut other = engine.begin(Isolation::Snapshot).unwrap();
    assert!(engine.reserve_begin_lock(&mut other).is_err());
    // The heir holds it (re-reserving is a no-op) until it ends.
    engine.reserve_begin_lock(&mut heir).unwrap();
    engine.rollback(heir).unwrap();
    engine.reserve_begin_lock(&mut other).unwrap();

    // Nothing to move from a transaction that holds no reservation.
    let mut plain = engine.begin(Isolation::Snapshot).unwrap();
    let mut next = engine.begin(Isolation::Snapshot).unwrap();
    assert!(!engine.transfer_begin_lock(&mut plain, &mut next));
    assert!(
        engine.reserve_begin_lock(&mut next).is_err(),
        "other holds it"
    );
    engine.commit(other).unwrap();
    engine.reserve_begin_lock(&mut next).unwrap();
}
