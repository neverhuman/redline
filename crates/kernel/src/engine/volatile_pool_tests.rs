//! A volatile (in-memory) engine keeps working once its pages outgrow the
//! buffer pool.
//!
//! Eviction writes a dirty page on its own only when crash recovery could
//! rebuild it, and otherwise asks the engine for a checkpoint. A volatile
//! engine has no WAL and no recovery, so it has nothing to checkpoint: every
//! page it logically "logged" stayed dirty for good, and the first allocation
//! past the pool size failed with "no unpinned frame available for eviction".
//! Its page file is private scratch space, so any unpinned page may be
//! written there and read back.

use tempfile::TempDir;

use super::buffer_eviction_tests::{
    ROWS, SMALL_POOL, assert_rows, config, create_indexed_table, insert_rows,
};
use super::{CommitDurability, Engine};
use crate::format::{PageGeneration, PageId, RowId, TuplePtr};
use crate::index::IndexRowRef;
use crate::txn::Isolation;

fn volatile_engine(temp: &TempDir) -> std::sync::Arc<Engine> {
    Engine::create_volatile(temp.path(), config(CommitDurability::Normal, SMALL_POOL)).unwrap()
}

#[test]
fn a_volatile_engine_holds_more_rows_than_its_pool() {
    let temp = TempDir::new().unwrap();
    let engine = volatile_engine(&temp);
    let rows = insert_rows(&engine, 0..ROWS).unwrap_or_else(|err| {
        panic!("inserting past a {SMALL_POOL}-page volatile pool failed: {err:?}")
    });
    assert!(engine.buffer.stats().evictions > 0, "nothing was evicted");
    assert_rows(&engine, &rows);

    // Updates and deletes append versions and undo records as well.
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    for (row, i) in rows.iter().take(ROWS / 2) {
        engine
            .update(
                &mut tx,
                *row,
                super::buffer_eviction_tests::payload(i + 1000),
            )
            .unwrap();
    }
    engine.commit(tx).unwrap();
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    for (row, i) in rows.iter().take(ROWS / 2) {
        assert_eq!(
            engine.get(&mut tx, *row).unwrap(),
            Some(super::buffer_eviction_tests::payload(i + 1000))
        );
    }
    assert_rows(&engine, &rows[ROWS / 2..]);
}

#[test]
fn a_volatile_index_grows_past_its_pool() {
    let temp = TempDir::new().unwrap();
    let engine = volatile_engine(&temp);
    let index_id = create_indexed_table(&engine);
    let index = engine.index_handle(index_id).unwrap();
    let key = |i: usize| {
        let mut key = format!("{:08}", (i * 7919) % 600).into_bytes();
        key.resize(200, b'k');
        key
    };
    let row = |i: usize| {
        IndexRowRef::with_row_id(
            RowId(i as u64 + 1),
            TuplePtr::new_with_generation(PageId(1), i as u16, PageGeneration::ONE),
        )
    };
    for i in 0..600 {
        let tx = engine.begin(Isolation::Snapshot).unwrap();
        index
            .insert_tx(tx.id(), &key(i), row(i))
            .unwrap_or_else(|err| panic!("index insert {i} failed: {err:?}"));
        engine.commit(tx).unwrap();
    }
    assert!(engine.buffer.stats().evictions > 0, "nothing was evicted");
    for i in 0..600 {
        assert_eq!(
            index.point_lookup(&key(i)).unwrap(),
            vec![row(i)],
            "key {i}"
        );
    }
    assert_eq!(index.validate().unwrap().errors, Vec::<&str>::new());
}
