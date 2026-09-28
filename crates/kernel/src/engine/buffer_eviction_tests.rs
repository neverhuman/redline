//! Buffer pool eviction keeps the WAL ahead of the page file.
//!
//! A dirty page may leave the pool only once the WAL records behind its page
//! LSN are durable. These tests fill a small pool with dirty pages and check
//! every page write against the WAL's durable LSN. The page-write hook is per
//! thread, and eviction runs on the thread that asked for a frame.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use tempfile::TempDir;

use super::{CommitDurability, Engine, EngineConfig};
use crate::catalog::{
    ColumnSpec, CreateIndexSpec, CreateTableSpec, DbName, IndexColumnSpec, IndexId, IndexOrigin,
    QualifiedName, SortDir,
};
use crate::format::{Lsn, PageGeneration, PageId, PageKind, RelId, RowId, TuplePtr};
use crate::index::IndexRowRef;
use crate::storage::buffer_test_hooks::{
    fail_next_page_file_sync, page_file_syncs, set_before_page_write_hook, set_before_pin_lock_hook,
};
use crate::storage::{BufferPool, PageFile};
use crate::txn::Isolation;
use crate::wal::{WalConfig, flushed_all_through, reset_flushed_all_through};
use crate::{Error, Result};

pub(super) const SMALL_POOL: usize = 16;
const PAGE_SIZE: usize = 4096;
/// 1 KiB rows fill about 40 heap pages of 4 KiB, well past `SMALL_POOL`.
pub(super) const ROWS: usize = 120;
/// 200-byte keys fill about 40 index leaves.
const KEYS: usize = 600;

const EVERY_DURABILITY: [CommitDurability; 3] = [
    CommitDurability::Strict,
    CommitDurability::Normal,
    CommitDurability::UnsafeDev,
];

pub(super) fn config(durability: CommitDurability, pool_pages: usize) -> EngineConfig {
    EngineConfig {
        rel_id: RelId(1),
        commit_durability: durability,
        busy_timeout: Duration::from_millis(250),
        heap_lanes: 1,
        page_size: PAGE_SIZE,
        buffer_pool_pages: pool_pages,
        wal: WalConfig {
            group_commit_delay_us: 0,
            ..WalConfig::default()
        },
        ..EngineConfig::default()
    }
}

pub(super) fn payload(i: usize) -> Vec<u8> {
    let mut bytes = vec![(i % 251) as u8; 1024];
    bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
    bytes
}

/// (page, page LSN, durable WAL LSN) for each page written ahead of its WAL.
type EarlyWrites = Rc<RefCell<Vec<(PageId, Lsn, Lsn)>>>;

/// Record every page this thread writes while the WAL is not yet durable
/// through that page's LSN.
fn watch_page_writes(engine: &Engine) -> EarlyWrites {
    let early = EarlyWrites::default();
    let sink = Rc::clone(&early);
    let wal = Arc::clone(&engine.wal);
    set_before_page_write_hook(Some(Box::new(move |page_id, page_lsn| {
        let durable = wal.durable_lsn().unwrap_or(Lsn::ZERO);
        if page_lsn > durable {
            sink.borrow_mut().push((page_id, page_lsn, durable));
        }
    })));
    early
}

/// Insert each row in its own committed transaction.
pub(super) fn insert_rows(engine: &Engine, range: Range<usize>) -> Result<Vec<(RowId, usize)>> {
    let mut rows = Vec::with_capacity(range.len());
    for i in range {
        let mut tx = engine.begin(Isolation::Snapshot)?;
        let row = engine.insert(&mut tx, payload(i))?;
        engine.commit(tx)?;
        rows.push((row, i));
    }
    Ok(rows)
}

pub(super) fn assert_rows(engine: &Engine, rows: &[(RowId, usize)]) {
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    for (row, i) in rows {
        assert_eq!(
            engine.get(&mut tx, *row).unwrap(),
            Some(payload(*i)),
            "row {row:?}"
        );
    }
}

#[test]
fn buffer_eviction_in_normal_mode_syncs_the_wal_before_the_page_write() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config(CommitDurability::Normal, SMALL_POOL)).unwrap();
    engine.enable_pool_pressure_checkpoints().unwrap();
    let early = watch_page_writes(&engine);
    let inserted = insert_rows(&engine, 0..ROWS);
    set_before_page_write_hook(None);
    let rows = inserted.unwrap();

    let stats = engine.buffer.stats();
    assert!(
        stats.evictions > 0 && stats.writes > 0,
        "the pool never wrote a dirty page out: {stats:?}"
    );
    assert_eq!(
        early.borrow().as_slice(),
        &[],
        "pages written before their WAL was durable (page, page lsn, durable lsn)"
    );
    assert_rows(&engine, &rows);
}

#[test]
fn buffer_eviction_admits_a_page_into_a_full_dirty_pool_in_every_durability_mode() {
    for durability in EVERY_DURABILITY {
        let temp = TempDir::new().unwrap();
        let engine = Engine::create(temp.path(), config(durability, SMALL_POOL)).unwrap();
        engine.enable_pool_pressure_checkpoints().unwrap();
        let early = watch_page_writes(&engine);
        let inserted = insert_rows(&engine, 0..ROWS);
        set_before_page_write_hook(None);
        let rows = inserted.unwrap_or_else(|err| {
            panic!("{durability:?}: an insert into a pool of dirty pages failed: {err:?}")
        });

        assert!(
            engine.buffer.stats().evictions > 0,
            "{durability:?}: the pool never evicted"
        );
        assert_eq!(
            early.borrow().as_slice(),
            &[],
            "{durability:?}: pages written before their WAL was durable"
        );
        assert_rows(&engine, &rows);
    }
}

/// Visit `0..KEYS` in an order that spreads consecutive inserts over the
/// whole key range, so many leaves are dirty at once.
fn scattered(i: usize) -> usize {
    (i * 7919) % KEYS
}

#[test]
fn buffer_eviction_checkpoints_a_pool_full_of_dirty_index_pages_in_every_durability_mode() {
    // A dirty leaf with a logged change may not leave on its own, so a pool
    // of such leaves has nothing to evict. Eviction asks for a checkpoint,
    // which writes them as one cut, instead of failing the insert.
    for durability in EVERY_DURABILITY {
        let temp = TempDir::new().unwrap();
        let engine = Engine::create(temp.path(), config(durability, SMALL_POOL)).unwrap();
        engine.enable_pool_pressure_checkpoints().unwrap();
        let index_id = create_indexed_table(&engine);
        let index = engine.index_handle(index_id).unwrap();
        let early = watch_page_writes(&engine);
        let mut inserted = Ok(());
        for i in (0..KEYS).map(scattered) {
            let tx = engine.begin(Isolation::Snapshot).unwrap();
            inserted = index.insert_tx(tx.id(), &index_key(i), index_row(i));
            if inserted.is_err() {
                break;
            }
            engine.commit(tx).unwrap();
        }
        set_before_page_write_hook(None);
        inserted.unwrap_or_else(|err| {
            panic!("{durability:?}: an index insert into a pool of dirty leaves failed: {err:?}")
        });
        assert!(
            engine.checkpoint_info().unwrap().is_some(),
            "{durability:?}: no checkpoint made room"
        );
        assert_eq!(
            early.borrow().as_slice(),
            &[],
            "{durability:?}: pages written before their WAL was durable"
        );
        assert_index_keys(&engine, index_id);
        drop(index);
        drop(engine);

        // UnsafeDev may lose commits made after the last checkpoint.
        if durability != CommitDurability::UnsafeDev {
            let reopened = Engine::open(temp.path(), config(durability, 1024)).unwrap();
            assert_index_keys(&reopened, index_id);
        }
    }
}

#[test]
fn buffer_eviction_frees_index_pages_that_recovery_dirtied() {
    let temp = TempDir::new().unwrap();
    // The first run has room for every page, so nothing is evicted and each
    // leaf reaches the reopened pool only through WAL replay. Normal commits
    // keep the test fast; the shutdown flush still makes the WAL durable.
    let engine = Engine::create(temp.path(), config(CommitDurability::Normal, 1024)).unwrap();
    let index_id = create_indexed_table(&engine);
    let index = engine.index_handle(index_id).unwrap();
    for i in 0..KEYS {
        let tx = engine.begin(Isolation::Snapshot).unwrap();
        index
            .insert_tx(tx.id(), &index_key(i), index_row(i))
            .unwrap();
        engine.commit(tx).unwrap();
    }
    assert_eq!(engine.buffer.stats().evictions, 0);
    drop(index);
    drop(engine);

    // Replay installs more index pages than the pool holds, so the frames it
    // installed at LSN zero have to leave during recovery, and replay must
    // not later write an older image over a page it already evicted. Every
    // key must still be found through the small pool.
    let reopened = Engine::open(temp.path(), config(CommitDurability::Normal, SMALL_POOL))
        .unwrap_or_else(|err| panic!("recovery into a {SMALL_POOL}-page pool failed: {err:?}"));
    assert_index_keys(&reopened, index_id);
    assert!(reopened.buffer.stats().evictions > 0);

    // Commits after recovery keep evicting, and a second reopen sees them.
    reopened.enable_pool_pressure_checkpoints().unwrap();
    let rows = insert_rows(&reopened, 0..ROWS).unwrap();
    drop(reopened);
    let again = Engine::open(temp.path(), config(CommitDurability::Normal, SMALL_POOL)).unwrap();
    assert_index_keys(&again, index_id);
    assert_rows(&again, &rows);
}

#[test]
fn buffer_eviction_frees_leaves_that_replayed_scattered_keys() {
    // Keys inserted out of order land on most leaves after each leaf's last
    // split image, so replaying their inserts dirties far more leaves than
    // the reopened pool holds. Recovery cannot checkpoint, so those leaves
    // have to be pages eviction may write.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config(CommitDurability::Normal, 1024)).unwrap();
    let index_id = create_indexed_table(&engine);
    let index = engine.index_handle(index_id).unwrap();
    for i in (0..KEYS).map(scattered) {
        let tx = engine.begin(Isolation::Snapshot).unwrap();
        index
            .insert_tx(tx.id(), &index_key(i), index_row(i))
            .unwrap();
        engine.commit(tx).unwrap();
    }
    // Delete marks replay onto leaves the same way.
    let deleted = |i: usize| i.is_multiple_of(3);
    for i in (0..KEYS).map(scattered).filter(|i| deleted(*i)) {
        let tx = engine.begin(Isolation::Snapshot).unwrap();
        index
            .delete_mark_tx(tx.id(), &index_key(i), index_row(i))
            .unwrap();
        engine.commit(tx).unwrap();
    }
    assert_eq!(engine.buffer.stats().evictions, 0);
    drop(index);
    drop(engine);

    let reopened = Engine::open(temp.path(), config(CommitDurability::Normal, SMALL_POOL))
        .unwrap_or_else(|err| panic!("recovery into a {SMALL_POOL}-page pool failed: {err:?}"));
    assert!(reopened.buffer.stats().evictions > 0);
    let index = reopened.index_handle(index_id).unwrap();
    for i in 0..KEYS {
        let expected = if deleted(i) {
            vec![]
        } else {
            vec![index_row(i)]
        };
        assert_eq!(
            index.point_lookup(&index_key(i)).unwrap(),
            expected,
            "key {i}"
        );
    }
}

#[test]
fn buffer_eviction_after_open_finds_the_replayed_wal_durable() {
    // UnsafeDev skips the shutdown fsync, so the reopened WAL may be written
    // but not durable. Replay stamps heap pages with LSN zero, so no page
    // LSN says which WAL a replayed page needs, and eviction writes those
    // pages while replay runs. Open has to make the scanned WAL durable
    // before the first of those writes, not merely by the time it returns.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config(CommitDurability::UnsafeDev, 1024)).unwrap();
    let rows = insert_rows(&engine, 0..ROWS).unwrap();
    drop(engine);

    reset_flushed_all_through();
    let at_first_write = Rc::new(Cell::new(None));
    let sink = Rc::clone(&at_first_write);
    set_before_page_write_hook(Some(Box::new(move |_, _| {
        if sink.get().is_none() {
            sink.set(Some(flushed_all_through()));
        }
    })));
    let opened = Engine::open_with_recovery_report(
        temp.path(),
        config(CommitDurability::Strict, SMALL_POOL),
    );
    set_before_page_write_hook(None);
    let (reopened, report) = opened.unwrap();
    assert!(report.valid_end_lsn > Lsn::ZERO);
    assert!(
        reopened.buffer.stats().evictions > 0,
        "replay never evicted a page"
    );
    let durable_at_first_write = at_first_write.get().expect("replay never wrote a page");
    assert!(
        durable_at_first_write >= report.valid_end_lsn,
        "replay wrote a page while the WAL was durable only through {durable_at_first_write:?}, \
         scanned through {:?}",
        report.valid_end_lsn
    );
    assert_rows(&reopened, &rows);
}

#[test]
fn buffer_eviction_write_is_synced_by_the_next_checkpoint() {
    // Eviction writes a page with LSN zero, such as one replay dirtied,
    // without syncing the page file. A checkpoint records an LSN past the
    // records behind that page and prunes the WAL below it, so its flush has
    // to sync the evicted write even when it writes no page itself.
    let temp = TempDir::new().unwrap();
    let (pool, next) = pool_after_an_eviction_write(&temp);
    pool.flush_page(next, Lsn::ZERO).unwrap();

    let syncs = page_file_syncs();
    let flushed = pool.flush_dirty_batches(Lsn(u64::MAX), 64).unwrap();
    assert_eq!(flushed.flushed_pages, 0);
    assert!(
        page_file_syncs() > syncs,
        "the checkpoint flush left the evicted write unsynced"
    );
}

/// A pool of one frame whose only page, dirty at LSN zero, eviction wrote to
/// make room for a second page, as it writes pages replay leaves dirty.
/// Returns the pool and the second page's id.
fn pool_after_an_eviction_write(temp: &TempDir) -> (Arc<BufferPool>, PageId) {
    let file = Arc::new(PageFile::create(temp.path().join("data.redline"), PAGE_SIZE).unwrap());
    let pool = Arc::new(BufferPool::new(file, 1).unwrap());
    drop(pool.allocate(PageKind::Heap, RelId(1)).unwrap());
    let next = pool.allocate(PageKind::Heap, RelId(1)).unwrap();
    assert_eq!(pool.stats().evictions, 1);
    (Arc::clone(&pool), next.page_id())
}

#[test]
fn buffer_pin_never_lands_on_a_frame_eviction_removed() {
    // A pin finds a resident frame and only then locks it. If eviction
    // removes the frame in between, the pin must load the page again, not
    // pin the removed frame, where no flush would ever see its changes.
    let temp = TempDir::new().unwrap();
    let file = Arc::new(PageFile::create(temp.path().join("data.redline"), PAGE_SIZE).unwrap());
    let pool = Arc::new(BufferPool::new(Arc::clone(&file), 2).unwrap());
    let cold = pool.allocate(PageKind::Heap, RelId(1)).unwrap();
    let cold_id = cold.page_id();
    drop(cold);
    pool.flush_page(cold_id, Lsn(u64::MAX)).unwrap();
    let _held = pool.allocate(PageKind::Heap, RelId(1)).unwrap();

    // The only page eviction may take is `cold`, which the pin below has
    // already found.
    let evicting = Arc::clone(&pool);
    set_before_pin_lock_hook(Some(Box::new(move |_| {
        drop(evicting.allocate(PageKind::Heap, RelId(1)).unwrap());
    })));
    let pinned = pool.pin(cold_id).unwrap();
    set_before_pin_lock_hook(None);
    assert!(pool.stats().evictions > 0, "the hook evicted nothing");
    pinned.mark_dirty(Lsn(7)).unwrap();
    drop(pinned);

    pool.flush_all(Lsn(u64::MAX)).unwrap();
    assert_eq!(
        file.read_page(cold_id).unwrap().header().unwrap().page_lsn,
        Lsn(7),
        "a change made through the pin never reached the page file"
    );
}

#[test]
fn buffer_eviction_write_in_flight_is_synced_by_a_concurrent_checkpoint() {
    // A checkpoint flush that starts while eviction is still writing a page
    // must wait for that write before it syncs; otherwise it syncs first and
    // records its LSN over a write that may never reach the disk.
    let temp = TempDir::new().unwrap();
    let file = Arc::new(PageFile::create(temp.path().join("data.redline"), PAGE_SIZE).unwrap());
    let pool = Arc::new(BufferPool::new(file, 1).unwrap());
    drop(pool.allocate(PageKind::Heap, RelId(1)).unwrap());

    let (writing_tx, writing) = mpsc::channel();
    let (release, release_rx) = mpsc::channel::<()>();
    let evicting = {
        let pool = Arc::clone(&pool);
        thread::spawn(move || {
            set_before_page_write_hook(Some(Box::new(move |_, _| {
                writing_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })));
            let next = pool.allocate(PageKind::Heap, RelId(1)).unwrap();
            set_before_page_write_hook(None);
            // Clean, so the checkpoint below has nothing of its own to write.
            pool.flush_page(next.page_id(), Lsn::ZERO).unwrap();
        })
    };
    writing.recv_timeout(Duration::from_secs(10)).unwrap();

    let (done_tx, done) = mpsc::channel();
    let checkpoint = {
        let pool = Arc::clone(&pool);
        thread::spawn(move || {
            let syncs = page_file_syncs();
            pool.flush_dirty_batches(Lsn(u64::MAX), 64).unwrap();
            done_tx.send(()).unwrap();
            page_file_syncs() - syncs
        })
    };
    let finished_during_write = done.recv_timeout(Duration::from_millis(300)).is_ok();
    release.send(()).unwrap();
    evicting.join().unwrap();
    let synced = checkpoint.join().unwrap();
    assert!(
        !finished_during_write,
        "the checkpoint flush finished while eviction was still writing a page"
    );
    assert!(
        synced > 0,
        "the checkpoint flush never synced the page file"
    );
}

#[test]
fn buffer_eviction_write_stays_owed_a_sync_after_a_failed_one() {
    // A checkpoint whose sync fails has not made the evicted write durable,
    // so the next checkpoint must sync even though it writes no page.
    let temp = TempDir::new().unwrap();
    let (pool, next) = pool_after_an_eviction_write(&temp);
    pool.flush_page(next, Lsn::ZERO).unwrap();

    fail_next_page_file_sync();
    assert!(pool.flush_dirty_batches(Lsn(u64::MAX), 64).is_err());
    let syncs = page_file_syncs();
    let flushed = pool.flush_dirty_batches(Lsn(u64::MAX), 64).unwrap();
    assert_eq!(flushed.flushed_pages, 0);
    assert!(
        page_file_syncs() > syncs,
        "after a failed sync the next checkpoint flush left the evicted write unsynced"
    );
}

#[test]
fn buffer_eviction_keeps_a_heap_page_whose_undo_is_only_in_memory() {
    // An update appends its new version to a heap page and the old one to
    // an undo page, and only the heap change is logged. Writing that heap
    // page alone could leave the file pointing at undo it never received.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config(CommitDurability::Normal, 1024)).unwrap();
    let rows = insert_rows(&engine, 0..1).unwrap();
    let checkpoint = engine.checkpoint().unwrap();
    let mut open = engine.begin(Isolation::Snapshot).unwrap();
    engine.update(&mut open, rows[0].0, payload(7)).unwrap();
    let heap_page = engine
        .heap
        .head_for_relation(RelId(1), rows[0].0)
        .unwrap()
        .unwrap()
        .page_id;
    let durable = engine.wal.flush_all().unwrap();
    engine
        .buffer
        .flush_page_if_evictable(heap_page, durable)
        .unwrap();
    let on_disk = PageFile::open(&engine.data_path, PAGE_SIZE)
        .unwrap()
        .read_page(heap_page)
        .unwrap();
    assert!(
        on_disk.header().unwrap().page_lsn <= checkpoint.checkpoint_lsn,
        "eviction wrote the heap page holding the uncommitted update"
    );
    drop(open);
}

#[test]
fn buffer_eviction_leaves_one_copy_of_each_row_after_a_reopen() {
    // Recovery replays heap records into new versions. A heap page that
    // eviction wrote ahead of a checkpoint would keep its rows next to the
    // replayed copies, and a page scan would return both.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config(CommitDurability::Normal, SMALL_POOL)).unwrap();
    engine.enable_pool_pressure_checkpoints().unwrap();
    let rows = insert_rows(&engine, 0..ROWS).unwrap();
    drop(engine);

    let reopened = Engine::open(temp.path(), config(CommitDurability::Normal, SMALL_POOL)).unwrap();
    assert_eq!(scanned_row_tags(&reopened), (0..ROWS).collect::<Vec<_>>());
    assert_rows(&reopened, &rows);
}

#[test]
fn buffer_eviction_during_replay_leaves_one_copy_after_another_reopen() {
    // Replay writes rows into new heap pages, and eviction writes some of
    // them before open returns. Reopening again without a checkpoint would
    // replay the same records next to those copies.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config(CommitDurability::Normal, 1024)).unwrap();
    let rows = insert_rows(&engine, 0..ROWS).unwrap();
    assert_eq!(engine.buffer.stats().writes, 0);
    drop(engine);

    for reopen in 0..2 {
        let reopened =
            Engine::open(temp.path(), config(CommitDurability::Normal, SMALL_POOL)).unwrap();
        assert_eq!(
            scanned_row_tags(&reopened),
            (0..ROWS).collect::<Vec<_>>(),
            "reopen {reopen}"
        );
        assert_rows(&reopened, &rows);
    }
}

/// The payload tag of every committed row a page scan returns, sorted. The
/// scan covers every page in the file and every page a row's head is on,
/// which may not have reached the file yet.
pub(super) fn scanned_row_tags(engine: &Engine) -> Vec<usize> {
    let tx = engine.begin(Isolation::Snapshot).unwrap();
    let pages = engine
        .relation_entries(RelId(1))
        .unwrap()
        .iter()
        .map(|(_, ptr)| ptr.page_id.0)
        .fold(engine.heap_page_count().unwrap(), u64::max);
    let mut tags: Vec<usize> = engine
        .parallel_scan_page_range(
            tx.snapshot(),
            Some(tx.id()),
            PageId(1)..PageId(pages + 1),
            Some(RelId(1)),
            1,
            None,
        )
        .unwrap()
        .iter()
        .map(|row| u64::from_le_bytes(row.payload[..8].try_into().unwrap()) as usize)
        .collect();
    tags.sort_unstable();
    tags
}

#[test]
fn buffer_eviction_of_an_open_transaction_keeps_its_ids_from_new_ones() {
    // A checkpoint eviction asks for can write pages holding tuples of a
    // transaction that never commits. Recovery must not hand
    // that transaction id, or its row ids, to new work: a reused transaction
    // id would make the orphaned tuples visible once the new one commits.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config(CommitDurability::Normal, SMALL_POOL)).unwrap();
    engine.enable_pool_pressure_checkpoints().unwrap();
    insert_rows(&engine, 0..3).unwrap();
    let mut open = engine.begin(Isolation::Snapshot).unwrap();
    let open_id = open.id();
    let mut open_rows = Vec::new();
    for i in 3..ROWS {
        open_rows.push(engine.insert(&mut open, payload(i)).unwrap());
    }
    assert!(
        engine.buffer.stats().writes > 0,
        "no checkpoint wrote the open transaction's pages"
    );
    drop(open);
    drop(engine);

    let reopened = Engine::open(temp.path(), config(CommitDurability::Normal, SMALL_POOL)).unwrap();
    let mut tx = reopened.begin(Isolation::Snapshot).unwrap();
    let new_id = tx.id();
    let new_row = reopened.insert(&mut tx, b"new".to_vec()).unwrap();
    reopened.commit(tx).unwrap();
    assert!(
        new_id > open_id,
        "{new_id:?} reuses the uncommitted {open_id:?}"
    );
    assert!(
        open_rows.iter().all(|row| new_row > *row),
        "{new_row:?} reuses a row id of the uncommitted transaction"
    );

    let tx = reopened.begin(Isolation::Snapshot).unwrap();
    let pages = reopened.heap_page_count().unwrap();
    let scanned = reopened
        .parallel_scan_page_range(
            tx.snapshot(),
            Some(tx.id()),
            PageId(1)..PageId(pages + 1),
            Some(RelId(1)),
            1,
            None,
        )
        .unwrap();
    let open_payloads: Vec<Vec<u8>> = (3..ROWS).map(payload).collect();
    assert!(
        scanned
            .iter()
            .all(|row| !open_payloads.contains(&row.payload)),
        "a scan shows rows of the transaction that never committed"
    );
}

#[test]
fn buffer_eviction_never_tears_an_uncommitted_index_split() {
    // An open transaction splits leaves that hold committed keys. Eviction
    // must not write the old leaf, now pointing at a new right sibling, while
    // that sibling stays in memory: recovery skips the uncommitted split's
    // images, so the page file would name a sibling it never received.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config(CommitDurability::Normal, 1024)).unwrap();
    let index_id = create_indexed_table(&engine);
    let index = engine.index_handle(index_id).unwrap();
    for i in (0..KEYS).step_by(2) {
        let tx = engine.begin(Isolation::Snapshot).unwrap();
        index
            .insert_tx(tx.id(), &index_key(i), index_row(i))
            .unwrap();
        engine.commit(tx).unwrap();
    }
    engine.checkpoint().unwrap();
    let checkpointed_pages = engine.buffer.page_count().unwrap();
    let open = engine.begin(Isolation::Snapshot).unwrap();
    for i in (1..KEYS).step_by(2) {
        index
            .insert_tx(open.id(), &index_key(i), index_row(i))
            .unwrap();
    }

    // Offer every page that existed at the checkpoint to eviction, the way
    // a clock pass would reach the old leaves before their new siblings.
    let durable = engine.wal.flush_all().unwrap();
    for page in 1..=checkpointed_pages {
        engine
            .buffer
            .flush_page_if_evictable(PageId(page), durable)
            .unwrap();
    }
    drop(open);
    drop(index);
    drop(engine);

    let reopened = Engine::open(temp.path(), config(CommitDurability::Normal, 1024))
        .unwrap_or_else(|err| panic!("reopen after an evicted uncommitted split: {err:?}"));
    let index = reopened.index_handle(index_id).unwrap();
    for i in (0..KEYS).step_by(2) {
        assert_eq!(
            index.point_lookup(&index_key(i)).unwrap(),
            vec![index_row(i)],
            "committed key {i}"
        );
    }
}

#[test]
fn buffer_eviction_fails_closed_unless_pressure_checkpoints_are_enabled() {
    // A checkpoint skips a page another writer changes while it runs, yet
    // records an LSN past that page's committed rows, so the engine does not
    // start one under memory pressure on its own. A pool of dirty pages then
    // refuses the allocation, as it did on c1af369.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config(CommitDurability::Normal, SMALL_POOL)).unwrap();
    let inserted = insert_rows(&engine, 0..ROWS);
    assert_eq!(
        inserted.unwrap_err(),
        Error::CorruptPage("no unpinned frame available for eviction")
    );
    assert_eq!(engine.checkpoint_info().unwrap(), None);
    assert_eq!(engine.buffer.stats().pressure_checkpoints, 0);
}

#[test]
fn buffer_eviction_without_an_engine_keeps_a_page_with_a_logged_change() {
    // A pool no engine is attached to cannot ask for a checkpoint, so the
    // dirty page stays and allocation fails closed.
    let temp = TempDir::new().unwrap();
    let file = Arc::new(PageFile::create(temp.path().join("data.redline"), PAGE_SIZE).unwrap());
    let pool = BufferPool::new(file, 1).unwrap();
    let first = pool.allocate(PageKind::Heap, RelId(1)).unwrap();
    first.mark_dirty(Lsn(10)).unwrap();
    drop(first);
    assert_eq!(
        pool.allocate(PageKind::Heap, RelId(1)).unwrap_err(),
        Error::CorruptPage("no unpinned frame available for eviction")
    );
}

fn index_key(i: usize) -> Vec<u8> {
    let mut key = format!("{i:08}").into_bytes();
    key.resize(200, b'k');
    key
}

fn index_row(i: usize) -> IndexRowRef {
    IndexRowRef::with_row_id(
        RowId(i as u64 + 1),
        TuplePtr::new_with_generation(PageId(1), i as u16, PageGeneration::ONE),
    )
}

fn assert_index_keys(engine: &Engine, index_id: IndexId) {
    let index = engine.index_handle(index_id).expect("index handle");
    for i in 0..KEYS {
        assert_eq!(
            index.point_lookup(&index_key(i)).unwrap(),
            vec![index_row(i)],
            "key {i}"
        );
    }
}

pub(super) fn create_indexed_table(engine: &Engine) -> IndexId {
    let column = |name: &str| ColumnSpec {
        name: DbName::new(name),
        declared_type: Some("TEXT".to_owned()),
        constraints: vec![],
        collation: None,
        default_value: None,
        autoincrement: false,
        generated: None,
    };
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    engine
        .create_table(
            &mut tx,
            CreateTableSpec {
                schema: None,
                name: DbName::new("t"),
                if_not_exists: false,
                columns: vec![column("v")],
                constraints: vec![],
                strict: false,
                without_rowid: false,
                normalized_sql: Some("CREATE TABLE t (v TEXT)".to_owned()),
            },
        )
        .unwrap();
    engine.commit(tx).unwrap();

    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    let index = engine
        .create_index(
            &mut tx,
            CreateIndexSpec {
                schema: None,
                name: DbName::new("ix_v"),
                if_not_exists: false,
                table: QualifiedName {
                    schema: DbName::new("main"),
                    name: DbName::new("t"),
                },
                unique: false,
                columns: vec![IndexColumnSpec {
                    name: DbName::new("v"),
                    sort_dir: SortDir::Asc,
                    collation: None,
                    expr_sql: None,
                    expr_referenced_cols: Vec::new(),
                }],
                origin: IndexOrigin::User,
                normalized_sql: Some("CREATE INDEX ix_v ON t(v)".to_owned()),
                predicate_sql: None,
            },
        )
        .unwrap();
    engine.commit(tx).unwrap();
    index.index_id
}
