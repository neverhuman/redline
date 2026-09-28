//! Reopening after a checkpoint rebuilds the row directory from the heap
//! pages. Every update and delete appends a version and leaves the older
//! ones on their pages, so the rebuild has to pick each row's newest version
//! rather than the first one a page scan meets.

use std::time::Duration;

use tempfile::TempDir;

use super::{CommitDurability, Engine, EngineConfig};
use crate::format::{Csn, PageId, PageKind, RelId, RowId};
use crate::txn::Isolation;
use crate::wal::WalConfig;

const PAGE_SIZE: usize = 4096;

fn config() -> EngineConfig {
    EngineConfig {
        rel_id: RelId(1),
        commit_durability: CommitDurability::Normal,
        busy_timeout: Duration::from_millis(250),
        heap_lanes: 1,
        page_size: PAGE_SIZE,
        buffer_pool_pages: 1024,
        wal: WalConfig {
            group_commit_delay_us: 0,
            ..WalConfig::default()
        },
        ..EngineConfig::default()
    }
}

/// About three of these fill a 4 KiB heap page.
fn payload(tag: u8) -> Vec<u8> {
    vec![tag; 1024]
}

fn insert(engine: &Engine, tag: u8) -> RowId {
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    let row = engine.insert(&mut tx, payload(tag)).unwrap();
    engine.commit(tx).unwrap();
    row
}

fn page_of(engine: &Engine, row: RowId) -> PageId {
    engine
        .heap
        .head_for_relation(RelId(1), row)
        .unwrap()
        .unwrap()
        .page_id
}

/// The tag of `row`'s payload after a reopen, or `None` when it is gone.
fn reopen_and_get(temp: &TempDir, row: RowId) -> Option<u8> {
    let engine = Engine::open(temp.path(), config()).unwrap();
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    let payload = engine.get(&mut tx, row).unwrap()?;
    assert!(payload.iter().all(|byte| *byte == payload[0]));
    Some(payload[0])
}

#[test]
fn directory_load_keeps_an_update_on_a_later_page() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let row = insert(&engine, 1);
    for tag in 10..20 {
        insert(&engine, tag);
    }
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    engine.update(&mut tx, row, payload(2)).unwrap();
    engine.commit(tx).unwrap();
    assert!(
        page_of(&engine, row) > PageId(1),
        "the update stayed on page 1"
    );
    engine.checkpoint().unwrap();
    drop(engine);

    assert_eq!(reopen_and_get(&temp, row), Some(2));
}

#[test]
fn directory_load_keeps_a_delete_on_a_later_page() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let row = insert(&engine, 1);
    for tag in 10..20 {
        insert(&engine, tag);
    }
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    engine.delete(&mut tx, row).unwrap();
    engine.commit(tx).unwrap();
    assert!(
        page_of(&engine, row) > PageId(1),
        "the delete stayed on page 1"
    );
    engine.checkpoint().unwrap();
    drop(engine);

    assert_eq!(reopen_and_get(&temp, row), None);
}

#[test]
fn directory_load_skips_a_version_whose_transaction_never_committed() {
    // The open transaction's version shares the heap page with the committed
    // one. Its undo record sits on an undo page that never reached the file,
    // so recovery must not make that version the head and follow its undo
    // pointer.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let row = insert(&engine, 1);
    engine.checkpoint().unwrap();
    let mut open = engine.begin(Isolation::Snapshot).unwrap();
    engine.update(&mut open, row, payload(2)).unwrap();
    let heap_page = page_of(&engine, row);
    let durable = engine.wal.flush_all().unwrap();
    engine
        .buffer
        .flush_page_if_evictable(heap_page, durable)
        .unwrap();
    drop(open);
    drop(engine);

    assert_eq!(reopen_and_get(&temp, row), Some(1));
}

#[test]
fn directory_load_follows_undo_when_a_transaction_moves_to_a_lower_page() {
    // One transaction inserts a row on a high page, then updates it onto a
    // reused page with a lower id. Both versions carry its commit, so only the
    // undo link from the update to the insert says which one is newer.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let doomed: Vec<RowId> = (0..3).map(|tag| insert(&engine, tag)).collect();
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    for row in &doomed {
        engine.delete(&mut tx, *row).unwrap();
    }
    engine.commit(tx).unwrap();
    let low_page = page_of(&engine, doomed[0]);

    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    let row = engine.insert(&mut tx, payload(7)).unwrap();
    let high_page = page_of(&engine, row);
    assert!(high_page > low_page);
    // Two more rows fill the high page. Vacuum then hands the low page back
    // for reuse, so the update has to move there.
    for tag in 20..22 {
        let filler = engine.insert(&mut tx, payload(tag)).unwrap();
        assert_eq!(page_of(&engine, filler), high_page);
    }
    let vacuumed = engine.vacuum_with_horizon(Csn(u64::MAX)).unwrap();
    assert_eq!(vacuumed.dead_rows_removed, 3);
    engine.update(&mut tx, row, payload(8)).unwrap();
    assert_eq!(page_of(&engine, row), low_page, "the update did not reuse");
    engine.commit(tx).unwrap();
    engine.checkpoint().unwrap();
    let kind = engine
        .buffer
        .pin(low_page)
        .unwrap()
        .with_page(|page| page.header().map(|header| header.kind))
        .unwrap();
    assert_eq!(kind, PageKind::Heap);
    drop(engine);

    assert_eq!(reopen_and_get(&temp, row), Some(8));
}

#[test]
fn directory_load_follows_a_reinsert_onto_a_lower_page() {
    // One transaction deletes a row and inserts the same row id again, and
    // the new version lands on a reused page with a lower id than the
    // tombstone. All three versions carry its commit. Without an undo link
    // from the new version to the tombstone, the tombstone's later position
    // won and the row was gone after a reopen.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let doomed: Vec<RowId> = (0..3).map(|tag| insert(&engine, tag)).collect();
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    for row in &doomed {
        engine.delete(&mut tx, *row).unwrap();
    }
    engine.commit(tx).unwrap();
    let low_page = page_of(&engine, doomed[0]);

    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    let row = engine.insert(&mut tx, payload(7)).unwrap();
    let high_page = page_of(&engine, row);
    assert!(high_page > low_page);
    for tag in 20..22 {
        let filler = engine.insert(&mut tx, payload(tag)).unwrap();
        assert_eq!(page_of(&engine, filler), high_page);
    }
    let vacuumed = engine.vacuum_with_horizon(Csn(u64::MAX)).unwrap();
    assert_eq!(vacuumed.dead_rows_removed, 3);
    // The tombstone still fits on the high page; the new version does not.
    engine.delete(&mut tx, row).unwrap();
    assert_eq!(page_of(&engine, row), high_page, "the delete moved");
    engine.insert_with_row_id(&mut tx, row, payload(8)).unwrap();
    assert_eq!(
        page_of(&engine, row),
        low_page,
        "the reinsert did not reuse"
    );
    assert_eq!(engine.get(&mut tx, row).unwrap(), Some(payload(8)));
    engine.commit(tx).unwrap();
    engine.checkpoint().unwrap();
    drop(engine);

    assert_eq!(reopen_and_get(&temp, row), Some(8));
}

#[test]
fn a_reinserted_row_keeps_its_older_versions_for_older_snapshots() {
    // A reinsert replaces the row's current version, a tombstone, the way an
    // update does, so a snapshot from before the reinsert still reaches the
    // versions under it: the tombstone, and before that the original row.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let row = insert(&engine, 1);
    let mut before_delete = engine.begin(Isolation::Snapshot).unwrap();
    assert_eq!(
        engine.get(&mut before_delete, row).unwrap(),
        Some(payload(1))
    );

    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    engine.delete(&mut tx, row).unwrap();
    engine.commit(tx).unwrap();
    let mut after_delete = engine.begin(Isolation::Snapshot).unwrap();
    assert_eq!(engine.get(&mut after_delete, row).unwrap(), None);

    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    engine.insert_with_row_id(&mut tx, row, payload(2)).unwrap();
    engine.commit(tx).unwrap();

    assert_eq!(
        engine.get(&mut before_delete, row).unwrap(),
        Some(payload(1))
    );
    assert_eq!(engine.get(&mut after_delete, row).unwrap(), None);
    let mut now = engine.begin(Isolation::Snapshot).unwrap();
    assert_eq!(engine.get(&mut now, row).unwrap(), Some(payload(2)));
}
