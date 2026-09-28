use crate::Result;
use crate::engine::tx::ConcurrentTxStatus;
use crate::format::{Csn, Lsn, PageId, RelId, RowId, TuplePtr, TupleVersion, TxId, UndoPtr};
use crate::storage::{BufferPool, BufferPoolStats, FlushStats};
use crate::txn::{Snapshot, TupleVisibility};
use crate::wal::WalCoordinator;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

#[path = "page_heap/directory.rs"]
mod directory;
#[path = "page_heap/logged_changes.rs"]
mod logged_changes;
#[path = "page_heap/mutation.rs"]
mod mutation;
#[path = "page_heap/policy.rs"]
mod policy;
#[path = "page_heap/scan.rs"]
mod scan;
pub use scan::{HeapScanRow, ParallelScanDiagnostics, parallel_scan_diagnostics};

#[derive(Debug)]
pub struct PageBackedHeap {
    rel_id: RelId,
    row_dir: Vec<RwLock<HashMap<RowId, TuplePtr>>>,
    relation_row_dir: Vec<RwLock<HashMap<RelId, HashMap<RowId, TuplePtr>>>>,
    append_lanes: Vec<Mutex<AppendLane>>,
    reusable_heap_pages: Mutex<Vec<PageId>>,
    reusable_undo_pages: Mutex<Vec<PageId>>,
    buffer: Arc<BufferPool>,
    wal: Option<Arc<WalCoordinator>>,
    next_row: AtomicU64,
    /// Held shared by each logged heap change and exclusively by a
    /// checkpoint writing pages; see `logged_changes.rs`.
    logged_changes: RwLock<()>,
}

#[derive(Debug, Default)]
struct AppendLane {
    heap_page: Option<PageId>,
    undo_page: Option<PageId>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VacuumStats {
    pub rows_scanned: usize,
    pub chains_pruned: usize,
    pub undo_links_removed: usize,
    pub dead_rows_removed: usize,
    pub oldest_active_snapshot_csn: Csn,
}

#[derive(Clone, Copy, Debug)]
pub struct RelationWriteTarget {
    pub rel_id: RelId,
    pub row_id: RowId,
}

impl PageBackedHeap {
    pub fn new(rel_id: RelId, lanes: usize, buffer: Arc<BufferPool>) -> Result<Self> {
        Self::new_with_wal(rel_id, lanes, buffer, None)
    }

    pub fn new_with_wal(
        rel_id: RelId,
        lanes: usize,
        buffer: Arc<BufferPool>,
        wal: Option<Arc<WalCoordinator>>,
    ) -> Result<Self> {
        let lanes = lanes.max(1).min(u16::MAX as usize);
        let mut row_dir = Vec::with_capacity(lanes);
        let mut relation_row_dir = Vec::with_capacity(lanes);
        let mut append_lanes = Vec::with_capacity(lanes);
        for _ in 0..lanes {
            row_dir.push(RwLock::new(HashMap::new()));
            relation_row_dir.push(RwLock::new(HashMap::new()));
            append_lanes.push(Mutex::new(AppendLane::default()));
        }
        Ok(Self {
            rel_id,
            row_dir,
            relation_row_dir,
            append_lanes,
            reusable_heap_pages: Mutex::new(Vec::new()),
            reusable_undo_pages: Mutex::new(Vec::new()),
            buffer,
            wal,
            next_row: AtomicU64::new(1),
            logged_changes: RwLock::new(()),
        })
    }

    pub fn reserve_row_id(&self) -> RowId {
        RowId(self.next_row.fetch_add(1, Ordering::Relaxed))
    }

    /// Keep `row_id` from being handed out again.
    pub(crate) fn reserve_recovered_row_id(&self, row_id: RowId) {
        advance_atomic_past(&self.next_row, row_id.0);
    }

    pub fn lower_next_row(&self, next_row: u64) {
        let mut current = self.next_row.load(Ordering::SeqCst);
        while current > next_row {
            match self.next_row.compare_exchange(
                current,
                next_row,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
    }

    pub fn flush_all(&self, durable_lsn: Lsn) -> Result<()> {
        self.buffer.flush_all(durable_lsn)
    }

    pub fn flush_dirty_batches(&self, durable_lsn: Lsn, batch_pages: usize) -> Result<FlushStats> {
        self.buffer.flush_dirty_batches(durable_lsn, batch_pages)
    }

    /// Write every dirty page for a checkpoint whose WAL is durable through
    /// `durable`; see [`BufferPool::flush_dirty_for_checkpoint`]. Heap pages
    /// form a complete cut only while no logged heap change is in flight,
    /// which `Engine::checkpoint` arranges around this call.
    pub fn flush_dirty_for_checkpoint(
        &self,
        durable: Lsn,
        force_wal: &dyn Fn(Lsn) -> Result<Lsn>,
    ) -> Result<FlushStats> {
        self.buffer.flush_dirty_for_checkpoint(durable, force_wal)
    }

    pub fn resident_pages(&self) -> usize {
        self.buffer.resident_pages()
    }

    pub fn buffer_stats(&self) -> BufferPoolStats {
        self.buffer.stats()
    }

    /// Redo a heap page image whose WAL record ends at `lsn`; see
    /// [`BufferPool::redo_page_image`].
    pub fn redo_page_image(&self, page: crate::format::Page, lsn: Lsn) -> Result<()> {
        self.buffer.redo_page_image(page, lsn).map(drop)
    }

    pub fn page_count(&self) -> Result<u64> {
        self.buffer.page_count()
    }

    /// Every page allocated so far, including pages still only in the buffer
    /// pool. [`PageBackedHeap::page_count`] counts the page file alone.
    pub fn allocated_page_count(&self) -> Result<u64> {
        self.buffer.allocated_page_count()
    }

    /// WS-C3 R2: borrow the underlying buffer pool. The scan module
    /// uses this to pin pages from worker threads inside a
    /// `std::thread::scope`; we keep the field itself private so other
    /// code paths stay routed through the explicit helpers.
    pub(super) fn buffer_ref(&self) -> &BufferPool {
        &self.buffer
    }
}

fn encode_undo_ptr(page_id: PageId, slot: u16) -> UndoPtr {
    UndoPtr((page_id.0 << 16) | slot as u64)
}

fn decode_undo_ptr(ptr: UndoPtr) -> (PageId, u16) {
    (PageId(ptr.0 >> 16), (ptr.0 & 0xffff) as u16)
}

fn advance_atomic_past(value: &AtomicU64, seen: u64) {
    let target = seen.saturating_add(1);
    let mut current = value.load(Ordering::SeqCst);
    while current < target {
        match value.compare_exchange(current, target, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => break,
            Err(next) => current = next,
        }
    }
}

trait ConcurrentVisibility {
    fn visibility_concurrent(
        &self,
        tx_status: &ConcurrentTxStatus,
        snapshot: &Snapshot,
        owner: Option<TxId>,
    ) -> TupleVisibility;
}

impl ConcurrentVisibility for TupleVersion {
    fn visibility_concurrent(
        &self,
        tx_status: &ConcurrentTxStatus,
        snapshot: &Snapshot,
        owner: Option<TxId>,
    ) -> TupleVisibility {
        if !tx_status.is_tx_visible(self.begin_tx, snapshot, owner) {
            return TupleVisibility::Invisible;
        }
        if self.end_tx != TxId::ZERO && tx_status.is_tx_visible(self.end_tx, snapshot, owner) {
            return TupleVisibility::Invisible;
        }
        if self.flags & crate::format::TUPLE_FLAG_DELETED != 0 {
            TupleVisibility::Deleted
        } else {
            TupleVisibility::Visible
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::PageBackedHeap;
    use crate::format::{Lsn, PageId, PageKind, RelId, RowId, TxId};
    use crate::storage::{BufferPool, PageFile};
    use crate::wal::{WalConfig, WalCoordinator};

    #[test]
    fn older_page_image_does_not_replace_a_newer_heap_page() {
        let dir = tempfile::tempdir().unwrap();
        let page_file = Arc::new(PageFile::create(dir.path().join("data.redline"), 4096).unwrap());
        let buffer = Arc::new(BufferPool::new(Arc::clone(&page_file), 32).unwrap());
        let wal =
            Arc::new(WalCoordinator::create(dir.path().join("wal"), WalConfig::default()).unwrap());
        let heap = PageBackedHeap::new_with_wal(RelId(1), 1, buffer, Some(wal)).unwrap();
        let row = RowId(3);
        heap.insert_for_relation(TxId(1), RelId(1), row, b"kept".to_vec(), Lsn(1))
            .unwrap();
        let before = heap
            .buffer
            .pin(PageId(1))
            .unwrap()
            .with_page(|page| page.slot_count())
            .unwrap();
        let stale = crate::format::Page::new(4096, PageKind::Heap, PageId(1), RelId(1)).unwrap();
        heap.redo_page_image(stale, Lsn(1)).unwrap();
        let after = heap
            .buffer
            .pin(PageId(1))
            .unwrap()
            .with_page(|page| page.slot_count())
            .unwrap();
        assert_eq!(after, before);
        assert_eq!(before, 1);
        let newer = crate::format::Page::new(4096, PageKind::Heap, PageId(1), RelId(1)).unwrap();
        heap.redo_page_image(newer, Lsn(u64::MAX)).unwrap();
        let replaced = heap
            .buffer
            .pin(PageId(1))
            .unwrap()
            .with_page(|page| page.slot_count())
            .unwrap();
        assert_eq!(replaced, 0);
    }
}
