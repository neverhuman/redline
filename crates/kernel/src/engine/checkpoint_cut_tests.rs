//! A checkpoint records a complete cut (workplan R5).
//!
//! Recovery starts at the LSN a checkpoint records and the WAL below it is
//! pruned, so every change below that LSN has to be in the page file once
//! the control file lands. A page another writer changes after the
//! checkpoint chose its LSN still holds the older changes, and it has to
//! reach the file with them. Heap redo appends a replayed row as a new
//! version rather than onto the page it changed, so the page file must not
//! also hold a heap change that recovery replays: each row has to come back
//! exactly once.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use tempfile::TempDir;

use super::buffer_eviction_tests::create_indexed_table;
use super::maintenance::set_after_checkpoint_cut_hook;
use super::{CommitDurability, Engine, EngineConfig, RecoveryTarget};
use crate::format::{Lsn, PageGeneration, PageId, RelId, RowId, TuplePtr};
use crate::index::IndexRowRef;
use crate::storage::ControlFile;
use crate::txn::Isolation;
use crate::wal::{WalConfig, WalPayload, WalReader, WalRecordKind};

/// How long a test waits for a checkpoint before it calls the schedule a
/// deadlock.
const DEADLOCK_GUARD: Duration = Duration::from_secs(60);

fn config() -> EngineConfig {
    EngineConfig {
        rel_id: RelId(1),
        commit_durability: CommitDurability::Normal,
        heap_lanes: 1,
        wal: WalConfig {
            group_commit_delay_us: 0,
            ..WalConfig::default()
        },
        ..EngineConfig::default()
    }
}

fn row_bytes(tag: u64) -> Vec<u8> {
    let mut bytes = vec![(tag % 251) as u8; 64];
    bytes[..8].copy_from_slice(&tag.to_le_bytes());
    bytes
}

fn insert(engine: &Engine, tag: u64) -> RowId {
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    let row = engine.insert(&mut tx, row_bytes(tag)).unwrap();
    engine.commit(tx).unwrap();
    row
}

/// Run `during` on this thread once the next checkpoint has chosen its LSN
/// and before it writes a page, then return that checkpoint's control file.
fn checkpoint_with(engine: &Engine, during: impl FnOnce() + 'static) -> ControlFile {
    let mut during = Some(during);
    set_after_checkpoint_cut_hook(Some(Box::new(move || {
        if let Some(during) = during.take() {
            during();
        }
    })));
    let control = engine.checkpoint();
    set_after_checkpoint_cut_hook(None);
    control.unwrap()
}

/// LSN of the heap insert record that wrote `row`.
fn heap_insert_lsn(engine: &Engine, row: RowId) -> Lsn {
    WalReader::new(&engine.wal_dir, engine.config.wal.clone())
        .scan()
        .unwrap()
        .iter()
        .find(|record| {
            record.kind == WalRecordKind::PageDelta
                && matches!(
                    WalPayload::decode(&record.payload).unwrap(),
                    WalPayload::HeapInsert { row_id, .. } if row_id == row
                )
        })
        .expect("heap insert record")
        .lsn
}

/// How many committed copies of each row a scan of every heap page finds.
fn copies_on_pages(engine: &Engine) -> BTreeMap<RowId, usize> {
    let tx = engine.begin(Isolation::Snapshot).unwrap();
    let pages = engine
        .relation_entries(engine.rel_id)
        .unwrap()
        .iter()
        .map(|(_, ptr)| ptr.page_id.0)
        .fold(engine.heap_page_count().unwrap(), u64::max);
    let mut copies = BTreeMap::new();
    for row in engine
        .parallel_scan_page_range(
            tx.snapshot(),
            Some(tx.id()),
            PageId(1)..PageId(pages + 1),
            Some(engine.rel_id),
            1,
            None,
        )
        .unwrap()
    {
        *copies.entry(row.row_id).or_insert(0) += 1;
    }
    copies
}

/// Assert that `rows` read back with their tags and that no heap page holds a
/// second copy of any of them.
fn assert_rows_once(engine: &Engine, rows: &[(RowId, u64)]) {
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    for (row, tag) in rows {
        assert_eq!(
            engine.get(&mut tx, *row).unwrap(),
            Some(row_bytes(*tag)),
            "row {row:?}"
        );
    }
    let copies = copies_on_pages(engine);
    for (row, _) in rows {
        assert_eq!(copies.get(row), Some(&1), "copies of row {row:?}");
    }
}

fn heap_page_of(engine: &Engine, row: RowId) -> PageId {
    engine
        .heap
        .head_for_relation(engine.rel_id, row)
        .unwrap()
        .expect("row head")
        .page_id
}

#[test]
fn checkpoint_keeps_committed_change_on_page_dirtied_past_cut() {
    for commit_newer in [true, false] {
        let temp = TempDir::new().unwrap();
        let engine = Engine::create(temp.path(), config()).unwrap();
        let older = insert(&engine, 1);

        // After the cut, a second writer puts a row on the same heap page.
        let newer = Rc::new(Cell::new(None));
        let newer_slot = Rc::clone(&newer);
        let writer = Arc::clone(&engine);
        let control = checkpoint_with(&engine, move || {
            let mut tx = writer.begin(Isolation::Snapshot).unwrap();
            newer_slot.set(Some(writer.insert(&mut tx, row_bytes(2)).unwrap()));
            if commit_newer {
                writer.commit(tx).unwrap();
            } else {
                writer.rollback(tx).unwrap();
            }
        });
        let newer = newer.get().expect("the hook never ran");
        assert_eq!(heap_page_of(&engine, older), heap_page_of(&engine, newer));
        let newer_lsn = heap_insert_lsn(&engine, newer);
        assert!(
            newer_lsn >= control.checkpoint_lsn,
            "the newer row was written before the cut"
        );
        // The heap page went to the file with the newer row on it, so heap
        // redo starts past that row's record.
        assert!(control.heap_redo_lsn > newer_lsn, "{control:?}");
        drop(engine);

        let reopened = Engine::open(temp.path(), config()).unwrap();
        let mut rows = vec![(older, 1)];
        if commit_newer {
            rows.push((newer, 2));
        }
        assert_rows_once(&reopened, &rows);
        let mut tx = reopened.begin(Isolation::Snapshot).unwrap();
        if !commit_newer {
            assert_eq!(
                reopened.get(&mut tx, newer).unwrap(),
                None,
                "a rolled-back row came back"
            );
        }
    }
}

#[test]
fn checkpoint_keeps_committed_change_on_page_dirtied_past_cut_across_pruned_segments() {
    // Small WAL segments, so the checkpoint prunes segments that held the
    // older rows' records, and a small pool.
    let config = EngineConfig {
        buffer_pool_pages: 16,
        wal: WalConfig {
            segment_bytes: 4096,
            group_commit_delay_us: 0,
            ..WalConfig::default()
        },
        ..config()
    };
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config.clone()).unwrap();
    let mut rows: Vec<(RowId, u64)> = (0..200).map(|tag| (insert(&engine, tag), tag)).collect();
    let segments = || std::fs::read_dir(temp.path().join("wal")).unwrap().count();
    let before = segments();
    assert!(before > 4, "the WAL never rotated: {before} segments");

    let newer = Rc::new(Cell::new(None));
    let newer_slot = Rc::clone(&newer);
    let writer = Arc::clone(&engine);
    let control = checkpoint_with(&engine, move || {
        newer_slot.set(Some(insert(&writer, 1_000)));
    });
    let newer = newer.get().expect("the hook never ran");
    rows.push((newer, 1_000));
    assert_eq!(
        heap_page_of(&engine, rows[199].0),
        heap_page_of(&engine, newer)
    );
    assert!(heap_insert_lsn(&engine, newer) >= control.checkpoint_lsn);

    // A checkpoint prunes only below the previous generation's checkpoint
    // LSN, which the other control slot still names (workplan R6). So the
    // segments holding the older rows' records go at the next checkpoint,
    // which finds the same page dirtied past its own cut again. A
    // checkpoint that skipped that page both times would lose every older
    // row on it.
    let newest = Rc::new(Cell::new(None));
    let newest_slot = Rc::clone(&newest);
    let writer = Arc::clone(&engine);
    let second = checkpoint_with(&engine, move || {
        newest_slot.set(Some(insert(&writer, 1_001)));
    });
    let newest = newest.get().expect("the hook never ran");
    rows.push((newest, 1_001));
    assert_eq!(heap_page_of(&engine, newer), heap_page_of(&engine, newest));
    assert!(heap_insert_lsn(&engine, newest) >= second.checkpoint_lsn);
    assert!(
        segments() < before,
        "the checkpoints pruned no segment: {before} before, {} after",
        segments()
    );
    drop(engine);

    let reopened = Engine::open(temp.path(), config).unwrap();
    assert_rows_once(&reopened, &rows);
}

#[test]
fn checkpoint_keeps_index_entries_on_a_leaf_changed_past_cut() {
    // A B-tree leaf that takes an insert after the cut still holds the older
    // entry. Index redo is idempotent, so the checkpoint may write the leaf
    // with the newer entry as well; each key has to come back exactly once.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let index_id = create_indexed_table(&engine);
    let index = engine.index_handle(index_id).unwrap();
    let key = |i: u64| format!("key-{i:04}").into_bytes();
    let entry = |i: u64| {
        IndexRowRef::with_row_id(
            RowId(i + 1),
            TuplePtr::new_with_generation(PageId(1), i as u16, PageGeneration::ONE),
        )
    };
    let tx = engine.begin(Isolation::Snapshot).unwrap();
    index.insert_tx(tx.id(), &key(1), entry(1)).unwrap();
    engine.commit(tx).unwrap();

    let writer = Arc::clone(&engine);
    let hook_index = Arc::clone(&index);
    checkpoint_with(&engine, move || {
        let tx = writer.begin(Isolation::Snapshot).unwrap();
        hook_index.insert_tx(tx.id(), &key(2), entry(2)).unwrap();
        writer.commit(tx).unwrap();
    });
    drop(index);
    drop(engine);

    let reopened = Engine::open(temp.path(), config()).unwrap();
    let index = reopened.index_handle(index_id).unwrap();
    for i in [1, 2] {
        assert_eq!(
            index.point_lookup(&key(i)).unwrap(),
            vec![entry(i)],
            "key {i}"
        );
    }
}

#[test]
fn concurrent_checkpoints_never_regress_redo_lsn() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let (done_tx, done) = mpsc::channel();
    let scenario = {
        let engine = Arc::clone(&engine);
        thread::spawn(move || {
            let mut rows = vec![(insert(&engine, 1), 1)];
            // The first checkpoint waits after its cut for a second one to
            // start, which must not overtake it.
            let (started_tx, started) = mpsc::channel::<()>();
            let (overtaken_tx, overtaken) = mpsc::channel::<ControlFile>();
            let second = {
                let engine = Arc::clone(&engine);
                thread::spawn(move || {
                    started.recv().unwrap();
                    let control = engine.checkpoint().unwrap();
                    let _ = overtaken_tx.send(control);
                    control
                })
            };
            let writer = Arc::clone(&engine);
            let overtook = Rc::new(Cell::new(None));
            let overtook_slot = Rc::clone(&overtook);
            let first = checkpoint_with(&engine, move || {
                rows_after_cut(&writer);
                started_tx.send(()).unwrap();
                overtook_slot.set(overtaken.recv_timeout(Duration::from_millis(200)).ok());
            });
            let overtook = overtook.get();
            let second = second.join().unwrap();
            rows.push((insert(&engine, 3), 3));
            done_tx.send((first, second, overtook, rows)).unwrap();
        })
    };
    let (first, second, overtook, mut rows) = done
        .recv_timeout(DEADLOCK_GUARD)
        .expect("two checkpoints deadlocked");
    scenario.join().unwrap();
    assert_eq!(
        overtook, None,
        "the second checkpoint finished inside the first"
    );
    assert!(second.generation > first.generation);
    assert!(
        second.checkpoint_lsn >= first.checkpoint_lsn,
        "generation {} recorded {:?} after generation {} recorded {:?}",
        second.generation,
        second.checkpoint_lsn,
        first.generation,
        first.checkpoint_lsn
    );
    let reloaded = engine.checkpoint_info().unwrap().unwrap();
    assert_eq!(reloaded, second);
    rows.push((RowId(2), 2));
    drop(engine);

    let reopened = Engine::open(temp.path(), config()).unwrap();
    assert_rows_once(&reopened, &rows);
}

#[test]
fn recovery_to_an_lsn_below_the_heap_redo_lsn_is_refused() {
    // The page file holds heap changes up to the heap redo LSN, so recovery
    // cannot stop at an earlier LSN, even one at or past the checkpoint LSN.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    insert(&engine, 1);
    let writer = Arc::clone(&engine);
    let control = checkpoint_with(&engine, move || {
        insert(&writer, 2);
    });
    assert!(
        control.heap_redo_lsn > control.checkpoint_lsn,
        "{control:?}"
    );
    drop(engine);

    let early = Engine::open_with_recovery_target(
        temp.path(),
        config(),
        RecoveryTarget::Lsn(control.checkpoint_lsn),
    );
    assert_eq!(
        early.err(),
        Some(crate::Error::CorruptWal(
            "requested recovery target is older than checkpoint base"
        ))
    );
    let reopened = Engine::open_with_recovery_target(
        temp.path(),
        config(),
        RecoveryTarget::Lsn(control.heap_redo_lsn),
    )
    .unwrap();
    assert_rows_once(&reopened, &[(RowId(1), 1), (RowId(2), 2)]);
}

/// The row the first checkpoint's hook writes after its cut. It gets row id 2
/// because the scenario inserted row 1 just before.
fn rows_after_cut(engine: &Engine) {
    assert_eq!(insert(engine, 2), RowId(2));
}
