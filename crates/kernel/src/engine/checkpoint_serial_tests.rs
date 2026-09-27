//! Checkpoints run one at a time.
//!
//! A checkpoint picks its LSN, flushes pages, then records that LSN in a new
//! control-file generation and prunes the WAL below it. If a second
//! checkpoint ran while the first was between its LSN and its control write,
//! the second could prune the WAL below its newer LSN, and the first would
//! then record its older LSN in the newer generation. Recovery starts from
//! the newest generation and would need WAL that is gone.

use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use tempfile::TempDir;

use super::maintenance::set_after_checkpoint_cut_hook;
use super::{CommitDurability, Engine, EngineConfig};
use crate::format::RowId;
use crate::txn::Isolation;
use crate::wal::WalConfig;

fn config() -> EngineConfig {
    EngineConfig {
        commit_durability: CommitDurability::Normal,
        wal: WalConfig {
            group_commit_delay_us: 0,
            ..WalConfig::default()
        },
        ..EngineConfig::default()
    }
}

fn insert(engine: &Engine, tag: u8) -> RowId {
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    let row = engine.insert(&mut tx, vec![tag; 64]).unwrap();
    engine.commit(tx).unwrap();
    row
}

#[test]
fn a_later_checkpoint_generation_never_records_an_older_lsn() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let mut rows = vec![insert(&engine, 1)];

    // The first checkpoint stops right after choosing its LSN.
    let (paused_tx, paused) = mpsc::channel();
    let (release, release_rx) = mpsc::channel::<()>();
    let first = {
        let engine = Arc::clone(&engine);
        thread::spawn(move || {
            set_after_checkpoint_cut_hook(Some(Box::new(move || {
                paused_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })));
            let control = engine.checkpoint();
            set_after_checkpoint_cut_hook(None);
            control.unwrap()
        })
    };
    paused.recv_timeout(Duration::from_secs(10)).unwrap();

    // More WAL, then a second checkpoint while the first is paused.
    rows.push(insert(&engine, 2));
    let (done_tx, done) = mpsc::channel();
    let second = {
        let engine = Arc::clone(&engine);
        thread::spawn(move || {
            let control = engine.checkpoint().unwrap();
            done_tx.send(()).unwrap();
            control
        })
    };
    let overtook = done.recv_timeout(Duration::from_millis(300)).is_ok();
    release.send(()).unwrap();
    let first = first.join().unwrap();
    let second = second.join().unwrap();

    let (earlier, later) = if first.generation < second.generation {
        (first, second)
    } else {
        (second, first)
    };
    assert!(
        later.checkpoint_lsn >= earlier.checkpoint_lsn,
        "generation {} recorded {:?} after generation {} recorded {:?}",
        later.generation,
        later.checkpoint_lsn,
        earlier.generation,
        earlier.checkpoint_lsn
    );
    assert!(
        !overtook,
        "a second checkpoint finished while the first was between its LSN and its control write"
    );
    drop(engine);

    let reopened = Engine::open(temp.path(), config()).unwrap();
    let mut tx = reopened.begin(Isolation::Snapshot).unwrap();
    for (row, tag) in rows.iter().zip([1_u8, 2]) {
        assert_eq!(reopened.get(&mut tx, *row).unwrap(), Some(vec![tag; 64]));
    }
}

/// Workplan R5, open: a checkpoint writes only pages at or below the LSN it
/// records. A page another writer changes after the checkpoint chose that
/// LSN is skipped whole, although it still holds committed rows older than
/// the LSN, whose WAL the checkpoint then prunes. Recovery starts at the
/// recorded LSN and never sees those rows again. This is why the engine
/// does not start checkpoints on its own under memory pressure.
#[test]
#[ignore = "R5: a checkpoint skips a page rewritten past its LSN and loses its older rows"]
fn checkpoint_keeps_a_committed_row_on_a_page_rewritten_past_its_lsn() {
    let temp = TempDir::new().unwrap();
    let config = EngineConfig {
        heap_lanes: 1,
        ..config()
    };
    let engine = Engine::create(temp.path(), config.clone()).unwrap();
    let older = insert(&engine, 1);

    let (paused_tx, paused) = mpsc::channel();
    let (release, release_rx) = mpsc::channel::<()>();
    let checkpoint = {
        let engine = Arc::clone(&engine);
        thread::spawn(move || {
            set_after_checkpoint_cut_hook(Some(Box::new(move || {
                paused_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })));
            let control = engine.checkpoint();
            set_after_checkpoint_cut_hook(None);
            control.unwrap()
        })
    };
    paused.recv_timeout(Duration::from_secs(10)).unwrap();
    // The same heap page takes a newer row while the checkpoint is paused.
    let newer = insert(&engine, 2);
    release.send(()).unwrap();
    checkpoint.join().unwrap();
    drop(engine);

    let reopened = Engine::open(temp.path(), config).unwrap();
    let mut tx = reopened.begin(Isolation::Snapshot).unwrap();
    assert_eq!(reopened.get(&mut tx, newer).unwrap(), Some(vec![2; 64]));
    assert_eq!(
        reopened.get(&mut tx, older).unwrap(),
        Some(vec![1; 64]),
        "the checkpoint lost a committed row on a page it skipped"
    );
}
