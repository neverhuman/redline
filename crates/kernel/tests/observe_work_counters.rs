//! Exact work counts for buffer pins, directory copies, and WAL signals.
//! This binary has one test so process-wide counters have no competing tests.

use std::sync::Arc;

use redlinedb_kernel::engine::page_heap::PageBackedHeap;
use redlinedb_kernel::engine::tx::ConcurrentTxStatus;
use redlinedb_kernel::format::{Lsn, PageKind, RelId, TxId};
use redlinedb_kernel::observe;
use redlinedb_kernel::storage::{BufferPool, PageFile};
use redlinedb_kernel::wal::WalRecordKind;
use redlinedb_kernel::wal::manager::{WalConfig, WalCoordinator};

#[test]
fn work_counters_match_buffer_directory_and_wal_events() {
    let dir = tempfile::tempdir().expect("temp dir");
    let file =
        Arc::new(PageFile::create(dir.path().join("pages.redline"), 4096).expect("page file"));
    let page_id = {
        let pool = BufferPool::new(Arc::clone(&file), 2).expect("seed pool");
        let guard = pool.allocate(PageKind::Heap, RelId(1)).expect("allocate");
        guard.mark_dirty(Lsn(1)).expect("dirty");
        let id = guard.page_id();
        drop(guard);
        pool.flush_all(Lsn(1)).expect("flush");
        id
    };

    let pool = BufferPool::new(file, 2).expect("cold pool");
    let before = observe::snapshot();
    for _ in 0..100 {
        drop(pool.pin(page_id).expect("pin"));
    }
    let pins = observe::snapshot().since(before);
    assert_eq!(pins.heap_page_pins, 100);
    assert_eq!(pins.frame_notifies, 1, "only the cold load notifies");
    assert_eq!(pins.frame_wakeups, 1);

    let page_file =
        Arc::new(PageFile::create(dir.path().join("heap.redline"), 4096).expect("heap file"));
    let heap_buffer = Arc::new(BufferPool::new(page_file, 32).expect("heap buffer"));
    let heap = PageBackedHeap::new(RelId(2), 8, heap_buffer).expect("heap");
    let txs = ConcurrentTxStatus::new();
    let tx = txs.begin();
    for _ in 0..50 {
        let row_id = heap.reserve_row_id();
        heap.insert_with_row_id(tx, row_id, b"x".to_vec(), Lsn(1))
            .expect("insert row");
    }
    let before = observe::snapshot();
    assert_eq!(heap.relation_rowids(RelId(2)).expect("row IDs").len(), 50);
    assert_eq!(
        observe::snapshot().since(before).directory_entries_copied,
        50
    );

    let wal = WalCoordinator::create(dir.path().join("wal"), WalConfig::default())
        .expect("WAL coordinator");
    let before = observe::snapshot();
    wal.append(WalRecordKind::PageDelta, TxId(1), b"payload".to_vec())
        .expect("append WAL record");
    assert_eq!(observe::snapshot().since(before).wal_writer_wakeups, 1);
}
