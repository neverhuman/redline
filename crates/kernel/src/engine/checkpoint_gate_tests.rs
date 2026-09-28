//! The heap checkpoint gate (workplan R5).
//!
//! A checkpoint reads its heap redo LSN and writes its pages while no
//! logged heap change is between its WAL append and its page install. A
//! heap append that got its record in after that LSN and its row onto a
//! page before the checkpoint wrote the page would reach the file with the
//! row on it, and recovery would replay the record again: two copies. The
//! gate makes such an append wait for the checkpoint's page writes.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tempfile::TempDir;

use super::Engine;
use super::checkpoint_cut_tests::{
    DEADLOCK_GUARD, assert_rows_once, config, heap_insert_lsn, heap_page_of, insert, row_bytes,
};
use crate::format::RowId;
use crate::storage::buffer_test_hooks::set_before_page_write_hook;
use crate::txn::Isolation;

/// How long an append that is not held back needs, with room to spare.
const UNBLOCKED_APPEND: Duration = Duration::from_millis(300);

fn insert_on_thread(engine: &Arc<Engine>, tag: u64, done: mpsc::Sender<RowId>) -> JoinHandle<()> {
    let engine = Arc::clone(engine);
    thread::spawn(move || {
        let mut tx = engine.begin(Isolation::Snapshot).unwrap();
        let row = engine.insert(&mut tx, row_bytes(tag)).unwrap();
        engine.commit(tx).unwrap();
        let _ = done.send(row);
    })
}

#[test]
fn a_heap_append_waits_while_a_checkpoint_writes_its_pages() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    // Older rows on two heap pages, so the checkpoint writes one page that
    // is not the one the next insert goes to.
    let mut rows = vec![(insert(&engine, 0), 0_u64)];
    let first_page = heap_page_of(&engine, rows[0].0);
    let mut tag = 1;
    while heap_page_of(&engine, rows.last().unwrap().0) == first_page {
        rows.push((insert(&engine, tag), tag));
        tag += 1;
    }
    let target = heap_page_of(&engine, rows.last().unwrap().0);
    let newer_tag = tag;

    let (done_tx, done_rx) = mpsc::channel();
    let done_rx = Rc::new(done_rx);
    let finished_during_writes = Rc::new(Cell::new(None));
    let writer = Rc::new(RefCell::new(None));
    let hook_engine = Arc::clone(&engine);
    let hook_rx = Rc::clone(&done_rx);
    let hook_finished = Rc::clone(&finished_during_writes);
    let hook_writer = Rc::clone(&writer);
    set_before_page_write_hook(Some(Box::new(move |page_id, _| {
        if hook_writer.borrow().is_some() || page_id == target {
            return;
        }
        hook_writer.replace(Some(insert_on_thread(
            &hook_engine,
            newer_tag,
            done_tx.clone(),
        )));
        hook_finished.set(Some(hook_rx.recv_timeout(UNBLOCKED_APPEND).ok()));
    })));
    let control = engine.checkpoint();
    set_before_page_write_hook(None);
    let control = control.unwrap();
    let during = finished_during_writes
        .take()
        .expect("the checkpoint wrote no page other than the insert target");
    assert_eq!(
        during, None,
        "a heap append finished while the checkpoint was writing pages"
    );
    let newer = done_rx
        .recv_timeout(DEADLOCK_GUARD)
        .expect("the held-back append never finished");
    writer.take().unwrap().join().unwrap();
    // The append went in after the checkpoint's heap redo LSN, so recovery
    // replays it and the page file does not hold it.
    assert!(
        heap_insert_lsn(&engine, newer) >= control.heap_redo_lsn,
        "{control:?}"
    );
    rows.push((newer, newer_tag));
    drop(engine);

    let reopened = Engine::open(temp.path(), config()).unwrap();
    assert_rows_once(&reopened, &rows);
}

#[test]
fn the_gate_holds_appends_and_the_wal_position_still() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    insert(&engine, 0);
    let (done_tx, done_rx) = mpsc::channel();
    let writer = {
        let quiesced = engine.heap.quiesce_logged_changes().unwrap();
        let before = engine.wal.reserved_lsn().unwrap();
        let writer = insert_on_thread(&engine, 1, done_tx);
        assert!(
            done_rx.recv_timeout(UNBLOCKED_APPEND).is_err(),
            "a heap append finished while the gate was held"
        );
        assert_eq!(
            engine.wal.reserved_lsn().unwrap(),
            before,
            "a heap append logged its record while the gate was held"
        );
        drop(quiesced);
        writer
    };
    done_rx
        .recv_timeout(DEADLOCK_GUARD)
        .expect("the append never finished after the gate opened");
    writer.join().unwrap();
}
