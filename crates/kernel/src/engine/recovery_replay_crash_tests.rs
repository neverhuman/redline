//! A crash during recovery must not leave a second copy of a replayed row.
//!
//! Heap replay puts each row into a new version on a new page and stamps the
//! page with LSN zero, so eviction may write it while recovery runs. A crash
//! before recovery ends leaves those pages in the file with no checkpoint
//! that covers them, and the next recovery replays the same records again.

use tempfile::TempDir;

use super::super::buffer_eviction_tests::{
    ROWS, SMALL_POOL, assert_rows, config, insert_rows, scanned_row_tags,
};
use super::super::{CommitDurability, Engine};
use super::replayed_pages::set_after_heap_replay_hook;
use crate::Error;

const CRASH: Error = Error::CorruptPage("test crash after heap replay");

fn page_file_len(temp: &TempDir) -> u64 {
    std::fs::metadata(
        temp.path()
            .join(config(CommitDurability::Normal, 1).data_file_name),
    )
    .unwrap()
    .len()
}

/// Open into a small pool and crash once heap replay is done. Returns how
/// far the page file grew.
fn crash_after_heap_replay(temp: &TempDir) -> u64 {
    let before = page_file_len(temp);
    set_after_heap_replay_hook(Some(Box::new(|| Err(CRASH))));
    let opened = Engine::open(temp.path(), config(CommitDurability::Normal, SMALL_POOL));
    set_after_heap_replay_hook(None);
    assert_eq!(opened.err(), Some(CRASH));
    page_file_len(temp) - before
}

fn assert_each_row_once_after_reopens(temp: &TempDir, rows: &[(crate::format::RowId, usize)]) {
    for reopen in 0..2 {
        let reopened =
            Engine::open(temp.path(), config(CommitDurability::Normal, SMALL_POOL)).unwrap();
        assert_eq!(
            scanned_row_tags(&reopened),
            (0..ROWS).collect::<Vec<_>>(),
            "reopen {reopen}"
        );
        assert_rows(&reopened, rows);
    }
}

#[test]
fn a_crash_during_replay_after_eviction_leaves_one_copy_of_each_row() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config(CommitDurability::Normal, 1024)).unwrap();
    let rows = insert_rows(&engine, 0..ROWS).unwrap();
    assert_eq!(engine.buffer.stats().writes, 0);
    drop(engine);

    // Two recoveries in a row die after eviction wrote replayed pages.
    for attempt in 0..2 {
        assert!(
            crash_after_heap_replay(&temp) > 0 || attempt > 0,
            "replay wrote no page before the crash"
        );
    }
    assert_each_row_once_after_reopens(&temp, &rows);
}

#[test]
fn a_crash_during_replay_after_a_checkpoint_leaves_one_copy_of_each_row() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config(CommitDurability::Normal, 1024)).unwrap();
    let mut rows = insert_rows(&engine, 0..ROWS / 2).unwrap();
    engine.checkpoint().unwrap();
    rows.extend(insert_rows(&engine, ROWS / 2..ROWS).unwrap());
    drop(engine);

    assert!(crash_after_heap_replay(&temp) > 0, "replay wrote no page");
    assert_each_row_once_after_reopens(&temp, &rows);
}

#[test]
fn eviction_after_a_recovery_that_took_no_checkpoint_leaves_one_copy_of_each_row() {
    // Replay that fits in the pool writes nothing and takes no checkpoint,
    // so the replayed pages stay dirty at LSN zero. Eviction may write them
    // later; a crash before the next checkpoint leaves them in the file.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config(CommitDurability::Normal, 1024)).unwrap();
    let rows = insert_rows(&engine, 0..ROWS).unwrap();
    drop(engine);

    let reopened = Engine::open(temp.path(), config(CommitDurability::Normal, 1024)).unwrap();
    assert_eq!(reopened.buffer.stats().writes, 0);
    assert_eq!(reopened.checkpoint_info().unwrap(), None);
    let before = page_file_len(&temp);
    let pages = reopened
        .relation_entries(crate::format::RelId(1))
        .unwrap()
        .iter()
        .map(|(_, ptr)| ptr.page_id.0)
        .max()
        .unwrap();
    for page in 1..=pages {
        reopened
            .buffer
            .flush_page_if_evictable(crate::format::PageId(page), crate::format::Lsn::ZERO)
            .unwrap();
    }
    assert!(page_file_len(&temp) > before, "eviction wrote no page");
    drop(reopened);

    assert_each_row_once_after_reopens(&temp, &rows);
}
