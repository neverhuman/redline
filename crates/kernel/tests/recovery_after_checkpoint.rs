//! Changes after a checkpoint recover on top of the checkpointed rows.
//!
//! Recovery loads the row directory from the checkpointed heap pages before
//! heap redo, so redo finds each row it changes through the directory. These
//! cases put a row's versions on both sides of the checkpoint: a change
//! written before it and committed after it, several changes after it, a
//! row id deleted before it and inserted again after it, and changes that
//! never commit.

use std::sync::Arc;
use std::time::Duration;

use redlinedb_kernel::engine::{CommitDurability, Engine, EngineConfig};
use redlinedb_kernel::format::{DEFAULT_PAGE_SIZE, RelId, RowId};
use redlinedb_kernel::txn::Isolation;
use redlinedb_kernel::wal::WalConfig;
use tempfile::TempDir;

const REL: RelId = RelId(1);

fn config() -> EngineConfig {
    EngineConfig {
        rel_id: REL,
        wal: WalConfig::default(),
        commit_durability: CommitDurability::Strict,
        lock_shards: 32,
        busy_timeout: Duration::from_millis(250),
        heap_lanes: 4,
        page_size: DEFAULT_PAGE_SIZE,
        buffer_pool_pages: 256,
        data_file_name: "data.redline".to_owned(),
    }
}

/// A checkpointed engine holding `rows` committed rows `"v1:<n>"`.
fn checkpointed(temp: &TempDir, rows: u64) -> (Arc<Engine>, Vec<RowId>) {
    let engine = Engine::create(temp.path(), config()).expect("create");
    let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
    let ids = (0..rows)
        .map(|n| engine.insert(&mut tx, format!("v1:{n}").into_bytes()))
        .collect::<Result<Vec<_>, _>>()
        .expect("insert");
    engine.commit(tx).expect("commit");
    engine.checkpoint().expect("checkpoint");
    (engine, ids)
}

fn reopen(temp: &TempDir, engine: Arc<Engine>) -> Arc<Engine> {
    drop(engine);
    Engine::open(temp.path(), config()).expect("reopen")
}

fn value(engine: &Engine, row: RowId) -> Option<String> {
    let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
    engine
        .get(&mut tx, row)
        .expect("get")
        .map(|bytes| String::from_utf8(bytes).expect("utf8"))
}

#[test]
fn a_change_written_before_the_checkpoint_and_committed_after_it_survives() {
    let temp = TempDir::new().expect("temp dir");
    let (engine, ids) = checkpointed(&temp, 50);
    let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
    engine
        .update(&mut tx, ids[10], b"v2:10".to_vec())
        .expect("update");
    engine.delete(&mut tx, ids[20]).expect("delete");
    // The checkpoint writes the pages holding the open transaction's
    // versions; its commit record lands after the checkpoint.
    engine
        .checkpoint()
        .expect("checkpoint with the change open");
    engine.commit(tx).expect("commit");
    let engine = reopen(&temp, engine);
    assert_eq!(value(&engine, ids[10]).as_deref(), Some("v2:10"));
    assert_eq!(value(&engine, ids[20]), None);
    assert_eq!(value(&engine, ids[30]).as_deref(), Some("v1:30"));
}

#[test]
fn several_changes_to_one_row_after_the_checkpoint_recover_the_last() {
    let temp = TempDir::new().expect("temp dir");
    let (engine, ids) = checkpointed(&temp, 50);
    for version in 2..=6 {
        let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
        engine
            .update(&mut tx, ids[7], format!("v{version}:7").into_bytes())
            .expect("update");
        engine.commit(tx).expect("commit");
    }
    let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
    engine.delete(&mut tx, ids[8]).expect("delete");
    engine.commit(tx).expect("commit");
    let engine = reopen(&temp, engine);
    assert_eq!(value(&engine, ids[7]).as_deref(), Some("v6:7"));
    assert_eq!(value(&engine, ids[8]), None);
}

#[test]
fn a_row_id_deleted_before_the_checkpoint_and_inserted_after_it_recovers() {
    let temp = TempDir::new().expect("temp dir");
    let (engine, ids) = checkpointed(&temp, 50);
    let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
    engine.delete(&mut tx, ids[5]).expect("delete");
    engine.commit(tx).expect("commit");
    engine.checkpoint().expect("checkpoint after the delete");
    let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
    engine
        .insert_for_relation(&mut tx, REL, ids[5], b"again:5".to_vec())
        .expect("insert the row id again");
    engine.commit(tx).expect("commit");
    let engine = reopen(&temp, engine);
    assert_eq!(value(&engine, ids[5]).as_deref(), Some("again:5"));
}

#[test]
fn rows_inserted_and_changed_after_the_checkpoint_recover() {
    let temp = TempDir::new().expect("temp dir");
    let (engine, ids) = checkpointed(&temp, 50);
    let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
    let fresh = engine.insert(&mut tx, b"new:1".to_vec()).expect("insert");
    engine.commit(tx).expect("commit");
    let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
    engine
        .update(&mut tx, fresh, b"new:2".to_vec())
        .expect("update");
    engine
        .update(&mut tx, ids[0], b"v2:0".to_vec())
        .expect("update");
    engine.commit(tx).expect("commit");
    let engine = reopen(&temp, engine);
    assert_eq!(value(&engine, fresh).as_deref(), Some("new:2"));
    assert_eq!(value(&engine, ids[0]).as_deref(), Some("v2:0"));
}

#[test]
fn changes_that_never_commit_leave_the_checkpointed_rows() {
    let temp = TempDir::new().expect("temp dir");
    let (engine, ids) = checkpointed(&temp, 50);
    let mut committed = engine.begin(Isolation::Snapshot).expect("begin");
    engine
        .update(&mut committed, ids[1], b"v2:1".to_vec())
        .expect("update");
    engine.commit(committed).expect("commit");
    let mut open = engine.begin(Isolation::Snapshot).expect("begin");
    engine
        .update(&mut open, ids[1], b"lost:1".to_vec())
        .expect("update");
    engine
        .update(&mut open, ids[2], b"lost:2".to_vec())
        .expect("update");
    engine.delete(&mut open, ids[3]).expect("delete");
    // The engine goes away with the transaction still open: no commit
    // record, as after a crash.
    std::mem::forget(open);
    let engine = reopen(&temp, engine);
    assert_eq!(value(&engine, ids[1]).as_deref(), Some("v2:1"));
    assert_eq!(value(&engine, ids[2]).as_deref(), Some("v1:2"));
    assert_eq!(value(&engine, ids[3]).as_deref(), Some("v1:3"));
}
