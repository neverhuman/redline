//! Inspection, checkpoint, statistics, vacuum, and integrity entrypoints.

use std::collections::HashMap;
use std::sync::Arc;

use crate::catalog::IndexId as CatalogIndexId;
use crate::engine::page_heap::{HeapScanRow, PageBackedHeap, ParallelScanDiagnostics, VacuumStats};
use crate::format::{Csn, Lsn, PageId, RelId, RowId, TuplePtr, TxId};
use crate::index::BtreeIndex;
use crate::storage::{
    BufferPool, BufferPoolStats, ControlFile, PageFile, PagePressureRelief, TxStatusCheckpoint,
};
use crate::telemetry::{Phase11Counters, Phase11CountersSnapshot};
use crate::txn::Snapshot;
use crate::txn::TxState;
use crate::{Error, Result};

use super::{CheckpointStats, ConcurrentTxStatus, Engine, StorageStatsSnapshot, TxStatusStats};

impl Engine {
    pub fn tx_state(&self, tx_id: TxId) -> TxState {
        self.txs.state(tx_id)
    }

    pub fn tx_status_stats(&self) -> TxStatusStats {
        self.txs.stats()
    }

    pub fn tx_status(&self) -> &ConcurrentTxStatus {
        &self.txs
    }

    #[doc(hidden)]
    pub fn buffer_pool_for_tests(&self) -> &BufferPool {
        &self.buffer
    }

    pub fn integrity_check(&self) -> Result<Vec<String>> {
        let snapshot = self.catalog.current();
        let mut errors = Vec::new();
        match PageFile::open(&self.data_path, self.config.page_size) {
            Ok(page_file) => match page_file.page_count() {
                Ok(page_count) => {
                    for page_no in 1..=page_count {
                        if let Err(err) = page_file.read_page(PageId(page_no)) {
                            errors.push(format!("page {page_no}: {err}"));
                        }
                    }
                }
                Err(err) => errors.push(format!("page file count: {err}")),
            },
            Err(err) => errors.push(format!("page file open: {err}")),
        }
        for index in &snapshot.indexes {
            if snapshot.table_by_id(index.table_id).is_none() {
                errors.push(format!(
                    "catalog index {} references missing table",
                    index.name
                ));
            }
        }
        if !self.volatile {
            let mut wal_reader = crate::wal::WalReader::new(&self.wal_dir, self.config.wal.clone());
            if let Err(err) = wal_reader.scan_report() {
                errors.push(format!("wal prefix scan: {err}"));
            }
        }
        let handles = self
            .index_handles
            .lock()
            .map_err(|_| Error::CorruptPage("engine index handles mutex poisoned"))?;
        for index in &snapshot.indexes {
            if index.meta_page_id.is_none() {
                continue;
            }
            let Some(handle) = handles.get(&index.index_id) else {
                errors.push(format!("index {} has no open handle", index.name));
                continue;
            };
            let report = handle.validate()?;
            for error in report.errors {
                errors.push(format!("index {}: {error}", index.name));
            }
        }
        Ok(errors)
    }

    pub fn oldest_active_snapshot_csn(&self) -> Csn {
        self.txs.oldest_active_snapshot_csn()
    }

    pub fn vacuum(&self) -> Result<VacuumStats> {
        self.vacuum_with_horizon(self.oldest_active_snapshot_csn())
    }

    pub fn vacuum_with_horizon(&self, horizon: Csn) -> Result<VacuumStats> {
        self.heap.vacuum(horizon, &self.txs)
    }

    pub fn flush_heap_pages(&self) -> Result<()> {
        if self.volatile {
            return Ok(());
        }
        let durable_lsn = self.wal.durable_lsn()?;
        self.heap.flush_all(durable_lsn)
    }

    pub fn resident_heap_pages(&self) -> usize {
        self.heap.resident_pages()
    }

    pub fn row_directory_entries(&self) -> Result<Vec<(RowId, TuplePtr)>> {
        self.heap.row_directory_entries()
    }

    pub fn relation_rowids(&self, rel_id: RelId) -> Result<Vec<RowId>> {
        self.heap.relation_rowids(rel_id)
    }

    /// WS-C3 R3: number of heap pages currently allocated in the
    /// page-backed heap. Surfaces the bound the SQL layer needs to
    /// drive [`Engine::parallel_scan_page_range`] without touching the
    /// private `heap` field. Counts every heap page including pages
    /// belonging to other relations; the scan itself filters by
    /// `rel_id`.
    pub fn heap_page_count(&self) -> Result<u64> {
        self.heap.page_count()
    }

    /// WS-C3 R3: thin Engine-level wrapper around
    /// [`PageBackedHeap::parallel_scan_page_range`]. R2-B shipped the
    /// scan API on the heap but did not expose an Engine accessor;
    /// R3-C (this commit) plumbs the access so the SQL covering-scan
    /// gate can actually dispatch when a per-database Rayon pool is
    /// installed and the downstream operator is HashAggregator or
    /// SpillSort. The wrapper performs no policy decisions — the SQL
    /// layer is expected to wrap this call in `pool.install(|| ...)`
    /// so workers run inside the dedicated pool's context.
    pub fn parallel_scan_page_range(
        &self,
        snapshot: &Snapshot,
        owner: Option<TxId>,
        page_range: std::ops::Range<PageId>,
        rel_filter: Option<RelId>,
        workers: usize,
        diagnostics: Option<&mut ParallelScanDiagnostics>,
    ) -> Result<Vec<HeapScanRow>> {
        self.heap.parallel_scan_page_range(
            &self.txs,
            snapshot,
            owner,
            page_range,
            rel_filter,
            workers,
            diagnostics,
        )
    }

    pub fn relation_entries(&self, rel_id: RelId) -> Result<Vec<(RowId, TuplePtr)>> {
        self.heap.relation_entries(rel_id)
    }

    pub fn buffer_pool_stats(&self) -> BufferPoolStats {
        self.heap.buffer_stats()
    }

    pub fn storage_stats(&self) -> Result<StorageStatsSnapshot> {
        Ok(StorageStatsSnapshot {
            buffer: self.heap.buffer_stats(),
            tx: self.txs.stats(),
            checkpoint: self.checkpoint_info()?,
            resident_heap_pages: self.heap.resident_pages(),
            wal_written_lsn: self.wal.written_lsn()?,
            wal_durable_lsn: self.wal.durable_lsn()?,
            vacuum_horizon_csn: self.oldest_active_snapshot_csn(),
            wal_sync_counters: self.wal.sync_counters_snapshot(),
            phase11_counters: self.phase11_counters_snapshot(),
        })
    }

    /// Phase 11 Wave 0: relaxed-atomic snapshot of the Phase 11
    /// counter aggregator. Mirrors
    /// [`crate::wal::WalCoordinator::sync_counters_snapshot`] for
    /// downstream telemetry callers (the bench harness picks it up
    /// via `Database::benchmark_stats`).
    pub fn phase11_counters_snapshot(&self) -> Phase11CountersSnapshot {
        self.phase11_counters.snapshot()
    }

    /// Phase 11 Wave 0: shared handle to the Phase 11 counter
    /// aggregator. Wave 1+ instrumentation sites can clone this
    /// `Arc` to gain direct access for `.fetch_add` calls without
    /// going through an additional accessor.
    pub fn phase11_counters(&self) -> Arc<Phase11Counters> {
        Arc::clone(&self.phase11_counters)
    }

    pub fn checkpoint(&self) -> Result<ControlFile> {
        self.checkpoint_with_stats().map(|stats| stats.control)
    }

    /// Write every dirty page and record where recovery starts (workplan R5).
    ///
    /// The control file records two LSNs. Below `checkpoint_lsn` every WAL
    /// record is reflected in the page file, recovery replays nothing, and
    /// the WAL may be pruned. It stops at the first index or commit record
    /// whose page install or publish is still in flight. Index and page-image
    /// redo skip what a page already holds, so index pages may also carry
    /// changes past it. Heap redo appends a replayed row as a new version and
    /// cannot tell what the file holds, so heap pages are written while no
    /// logged heap change is in flight, and `heap_redo_lsn` records the WAL
    /// position of that instant: the file holds every heap record below it and
    /// none above.
    ///
    /// Every dirty page is written, whatever its LSN, after the WAL is made
    /// durable through that LSN. Before, a page changed after the checkpoint
    /// chose its LSN was skipped whole, although it still held older changes
    /// whose WAL the checkpoint then pruned.
    pub fn checkpoint_with_stats(&self) -> Result<CheckpointStats> {
        if self.volatile {
            return Ok(CheckpointStats {
                control: ControlFile::default(),
                flushed_pages: 0,
                flush_batches: 0,
            });
        }
        // Serialize from the start: a checkpoint that chose its LSNs later
        // must not publish before one that chose them earlier.
        let _serial = self
            .checkpoint_serial
            .lock()
            .map_err(|_| Error::CorruptPage("checkpoint serial mutex poisoned"))?;
        let durable_lsn = self.wal.flush_all()?;
        // Do not record a checkpoint past a WAL record whose page image is
        // still unpublished. Recovery would skip that record.
        let checkpoint_lsn = self.wal.checkpoint_horizon(durable_lsn)?;
        #[cfg(test)]
        run_after_checkpoint_cut_hook();
        // A page LSN past every appended record is a placeholder, such as the
        // LSN an undo page carries; no record stands behind it.
        let force_wal = |page_lsn: Lsn| -> Result<Lsn> {
            let target = page_lsn.min(self.wal.reserved_lsn()?);
            self.wal.flush_until(target)
        };
        let (heap_redo_lsn, flush) = {
            let _quiesced = self.heap.quiesce_logged_changes()?;
            let heap_redo_lsn = self.wal.reserved_lsn()?.max(checkpoint_lsn);
            let flush = self
                .buffer
                .write_dirty_for_checkpoint(durable_lsn, &force_wal)?;
            (heap_redo_lsn, flush)
        };
        self.buffer.sync_checkpoint_writes(flush)?;
        // The control file names the heap redo LSN as where the next record
        // goes. That record must never land below it after a crash.
        self.wal.flush_until(heap_redo_lsn)?;
        let page_count = self.heap.page_count()?;
        let mut checkpoint = self
            .checkpoint
            .lock()
            .map_err(|_| Error::CorruptPage("checkpoint mutex poisoned"))?;
        let generation = checkpoint
            .map(|control| control.generation + 1)
            .unwrap_or(1);
        self.tx_status_store.write(&TxStatusCheckpoint {
            generation,
            next_tx: self.txs.next_tx(),
            next_csn: self.txs.next_csn(),
            published_csn: self.txs.published_csn(),
            entries: self.txs.committed_states(),
        })?;
        // The WAL pruned below this checkpoint holds the catalog snapshots
        // recovery would otherwise rebuild the schema from, so the saved
        // file must be durable even when commits currently skip fsync.
        self.catalog_store
            .save_durable(self.catalog.current().as_ref())?;
        // Lane E failpoint: armed before the new control-file generation lands
        // on disk. A crash here forces recovery to fall back to the previous
        // generation, exercising the dual-control-file protocol.
        crate::fail_point!("engine::checkpoint");
        let next =
            self.control
                .write_next(*checkpoint, checkpoint_lsn, heap_redo_lsn, page_count)?;
        self.wal
            .prune_segments_below_checkpoint_lsn(next.checkpoint_lsn)?;
        *checkpoint = Some(next);
        Ok(CheckpointStats {
            control: next,
            flushed_pages: flush.flushed_pages,
            flush_batches: flush.batches,
        })
    }

    /// Let the buffer pool run a checkpoint when every unpinned frame holds
    /// a dirty page it may not write on its own, instead of failing the
    /// allocation with "no unpinned frame available for eviction".
    ///
    /// Off by default, and safe only while one thread writes. A checkpoint
    /// does not yet take a complete cut (workplan R5): it skips a page that
    /// another writer changes between its WAL flush and its page flush, yet
    /// records an LSN past that page's earlier committed changes and prunes
    /// the WAL below it. A crash, or closing without a later checkpoint that
    /// writes the page, then loses those changes. Checkpoints the caller
    /// runs itself share that limit; this only makes them automatic under
    /// memory pressure. Use it only on an engine opened to the end of its
    /// WAL: a checkpoint after recovery to an earlier target records the WAL
    /// end. A volatile engine has nothing to checkpoint and ignores it.
    pub fn enable_pool_pressure_checkpoints(self: &Arc<Self>) -> Result<()> {
        if self.volatile {
            return Ok(());
        }
        let engine: std::sync::Weak<Self> = Arc::downgrade(self);
        let relief: std::sync::Weak<dyn PagePressureRelief> = engine;
        match self.buffer.attach_pressure_relief(relief) {
            // Already enabled.
            Ok(()) | Err(Error::CorruptPage("buffer pool already has pressure relief")) => Ok(()),
            Err(err) => Err(err),
        }
    }

    pub fn checkpoint_info(&self) -> Result<Option<ControlFile>> {
        self.checkpoint
            .lock()
            .map(|checkpoint| *checkpoint)
            .map_err(|_| Error::CorruptPage("checkpoint mutex poisoned"))
    }

    /// Lane INT: structural validation across every catalog index handle,
    /// returning the per-index `errors` lists for the
    /// `redline_index_check` PRAGMA. Complements the flat
    /// `integrity_check()` which returns errors as strings only.
    pub fn integrity_check_per_index(&self) -> Result<Vec<(String, Vec<String>)>> {
        let snapshot = self.catalog.current();
        let handles = self
            .index_handles
            .lock()
            .map_err(|_| Error::CorruptPage("engine index handles mutex poisoned"))?;
        let mut out = Vec::new();
        for index in &snapshot.indexes {
            let Some(btree) = handles.get(&index.index_id) else {
                continue;
            };
            let validation = btree.validate()?;
            let errors = validation
                .errors
                .iter()
                .map(|s| (*s).to_owned())
                .collect::<Vec<_>>();
            out.push((index.name.to_string(), errors));
        }
        Ok(out)
    }

    /// Lane INT: full heap/index/page equivalence check. Returns the
    /// structured [`crate::integrity::IntegrityReport`] consumed by the
    /// `redline_full_check` PRAGMA and the bench certification harness.
    pub fn integrity_check_full(&self) -> Result<crate::integrity::IntegrityReport> {
        crate::integrity::run_full(self)
    }

    pub(crate) fn buffer_for_integrity(&self) -> &Arc<BufferPool> {
        &self.buffer
    }

    pub(crate) fn heap_for_integrity(&self) -> &PageBackedHeap {
        &self.heap
    }

    pub(crate) fn txs_for_integrity(&self) -> &ConcurrentTxStatus {
        &self.txs
    }

    pub(crate) fn txs_snapshot_for_integrity(&self) -> crate::txn::Snapshot {
        self.txs.snapshot()
    }

    pub(crate) fn read_raw_page_bytes_for_integrity(&self, page_id: PageId) -> Result<Vec<u8>> {
        self.buffer.read_page_bytes_unchecked(page_id)
    }

    pub(crate) fn index_handles_for_integrity(
        &self,
    ) -> Result<HashMap<CatalogIndexId, Arc<BtreeIndex>>> {
        let handles = self
            .index_handles
            .lock()
            .map_err(|_| Error::CorruptPage("engine index handles mutex poisoned"))?;
        Ok(handles.clone())
    }
}

impl PagePressureRelief for Engine {
    fn relieve_page_pressure(&self) -> Result<bool> {
        if self.volatile {
            return Ok(false);
        }
        self.checkpoint_with_stats().map(|_| true)
    }
}

#[cfg(test)]
thread_local! {
    static AFTER_CHECKPOINT_CUT: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        std::cell::RefCell::new(None);
}

/// Test hook that runs on this thread once a checkpoint has chosen its LSN
/// and before it flushes a page.
#[cfg(test)]
pub(super) fn set_after_checkpoint_cut_hook(hook: Option<Box<dyn FnMut()>>) {
    AFTER_CHECKPOINT_CUT.with(|slot| *slot.borrow_mut() = hook);
}

#[cfg(test)]
fn run_after_checkpoint_cut_hook() {
    AFTER_CHECKPOINT_CUT.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook();
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tempfile::TempDir;

    use super::super::{Engine, EngineConfig};
    use crate::format::{PageGeneration, PageId, RelId, TuplePtr, TxId};
    use crate::index::{BtreeIndex, IndexDescriptor, IndexId, IndexRowRef, IndexUniqueness};
    use crate::txn::Isolation;
    use crate::wal::{
        WalPayload, WalReader, WalRecordKind, set_before_commit_publish_hook,
        set_before_page_install_hook,
    };

    #[test]
    fn checkpoint_does_not_pass_an_uninstalled_index_record() {
        let dir = TempDir::new().unwrap();
        let engine = Engine::create(
            dir.path(),
            EngineConfig {
                buffer_pool_pages: 32,
                wal: crate::wal::WalConfig {
                    group_commit_delay_us: 0,
                    ..crate::wal::WalConfig::default()
                },
                ..EngineConfig::default()
            },
        )
        .unwrap();
        let index = BtreeIndex::create_with_wal(
            Arc::clone(&engine.buffer),
            IndexDescriptor::new(IndexId(7), RelId(1), IndexUniqueness::NonUnique),
            engine.page_wal(),
        )
        .unwrap();
        let seen = Arc::new(Mutex::new(None));
        let seen_hook = Arc::clone(&seen);
        let hook_engine = Arc::clone(&engine);
        set_before_page_install_hook(Some(Box::new(move || {
            let checkpoint = hook_engine
                .checkpoint()
                .expect("checkpoint while the leaf is unpublished");
            *seen_hook.lock().expect("checkpoint lsn") = Some(checkpoint.checkpoint_lsn);
        })));
        index
            .insert_tx(
                TxId(1),
                b"k",
                IndexRowRef::new(TuplePtr::new_with_generation(
                    PageId(3),
                    1,
                    PageGeneration::ONE,
                )),
            )
            .unwrap();
        set_before_page_install_hook(None);

        let during = seen
            .lock()
            .expect("seen lsn")
            .expect("index insert did not checkpoint before install");
        let records = WalReader::new(&engine.wal_dir, engine.config.wal.clone())
            .scan()
            .unwrap();
        let insert_lsn = records
            .iter()
            .find(|record| {
                record.kind == WalRecordKind::PageDelta
                    && matches!(
                        WalPayload::decode(&record.payload).unwrap(),
                        WalPayload::IndexInsert { .. }
                    )
            })
            .expect("index insert wal record")
            .lsn;
        assert!(
            during <= insert_lsn,
            "checkpoint {during:?} passed unpublished index record {insert_lsn:?}"
        );
        let after = engine.checkpoint().unwrap();
        assert!(after.checkpoint_lsn > insert_lsn);
    }

    fn small_engine(dir: &std::path::Path) -> Arc<Engine> {
        Engine::create(
            dir,
            EngineConfig {
                buffer_pool_pages: 32,
                wal: crate::wal::WalConfig {
                    group_commit_delay_us: 0,
                    ..crate::wal::WalConfig::default()
                },
                ..EngineConfig::default()
            },
        )
        .unwrap()
    }

    fn checkpoint_while<R>(
        engine: &Arc<Engine>,
        body: impl FnOnce() -> R,
    ) -> (R, crate::format::Lsn) {
        let seen = Arc::new(Mutex::new(None));
        let seen_hook = Arc::clone(&seen);
        let hook_engine = Arc::clone(engine);
        set_before_page_install_hook(Some(Box::new(move || {
            let checkpoint = hook_engine
                .checkpoint()
                .expect("checkpoint while the page image is unpublished");
            *seen_hook.lock().expect("checkpoint lsn") = Some(checkpoint.checkpoint_lsn);
        })));
        let result = body();
        set_before_page_install_hook(None);
        let during = seen
            .lock()
            .expect("seen lsn")
            .expect("page publish did not checkpoint before install");
        (result, during)
    }

    #[test]
    fn checkpoint_does_not_pass_an_uninstalled_page_image() {
        let dir = TempDir::new().unwrap();
        let engine = small_engine(dir.path());
        let index = BtreeIndex::create_with_wal(
            Arc::clone(&engine.buffer),
            IndexDescriptor::new(IndexId(8), RelId(1), IndexUniqueness::NonUnique),
            engine.page_wal(),
        )
        .unwrap();
        let (_, during) = checkpoint_while(&engine, || {
            index.record_initial_page_images(TxId(2)).unwrap()
        });
        let records = WalReader::new(&engine.wal_dir, engine.config.wal.clone())
            .scan()
            .unwrap();
        let image_lsn = records
            .iter()
            .find(|record| {
                record.kind == WalRecordKind::PageImage
                    && matches!(
                        WalPayload::decode(&record.payload).unwrap(),
                        WalPayload::PageImage { .. }
                    )
            })
            .expect("page image wal record")
            .lsn;
        assert!(
            during <= image_lsn,
            "checkpoint {during:?} passed unpublished page image {image_lsn:?}"
        );
        let after = engine.checkpoint().unwrap();
        assert!(after.checkpoint_lsn > image_lsn);
    }

    #[test]
    fn checkpoint_does_not_pass_an_uninstalled_index_delete() {
        let dir = TempDir::new().unwrap();
        let engine = small_engine(dir.path());
        let index = BtreeIndex::create_with_wal(
            Arc::clone(&engine.buffer),
            IndexDescriptor::new(IndexId(9), RelId(1), IndexUniqueness::NonUnique),
            engine.page_wal(),
        )
        .unwrap();
        let row = IndexRowRef::new(TuplePtr::new_with_generation(
            PageId(4),
            1,
            PageGeneration::ONE,
        ));
        index.insert_tx(TxId(1), b"k", row).unwrap();
        let (_, during) = checkpoint_while(&engine, || {
            index.delete_mark_tx(TxId(3), b"k", row).unwrap();
        });
        let records = WalReader::new(&engine.wal_dir, engine.config.wal.clone())
            .scan()
            .unwrap();
        let delete_lsn = records
            .iter()
            .find(|record| {
                record.kind == WalRecordKind::PageDelta
                    && matches!(
                        WalPayload::decode(&record.payload).unwrap(),
                        WalPayload::IndexDelete { .. }
                    )
            })
            .expect("index delete wal record")
            .lsn;
        assert!(
            during <= delete_lsn,
            "checkpoint {during:?} passed unpublished index delete {delete_lsn:?}"
        );
        let after = engine.checkpoint().unwrap();
        assert!(after.checkpoint_lsn > delete_lsn);
    }

    #[test]
    fn checkpoint_during_commit_keeps_the_row_after_reopen() {
        let dir = TempDir::new().unwrap();
        let config = EngineConfig {
            buffer_pool_pages: 32,
            wal: crate::wal::WalConfig {
                group_commit_delay_us: 0,
                ..crate::wal::WalConfig::default()
            },
            ..EngineConfig::default()
        };
        let engine = Engine::create(dir.path(), config.clone()).unwrap();
        let mut tx = engine.begin(Isolation::Snapshot).unwrap();
        let row = engine.insert(&mut tx, b"alpha".to_vec()).unwrap();
        let hook_engine = Arc::clone(&engine);
        let seen = Arc::new(Mutex::new(None));
        let seen_hook = Arc::clone(&seen);
        set_before_commit_publish_hook(Some(Box::new(move || {
            let checkpoint = hook_engine
                .checkpoint()
                .expect("checkpoint before commit publish");
            *seen_hook.lock().expect("checkpoint lsn") = Some(checkpoint.checkpoint_lsn);
        })));
        engine.commit(tx).unwrap();
        set_before_commit_publish_hook(None);
        let during = seen
            .lock()
            .expect("seen lsn")
            .expect("commit did not checkpoint before publish");
        drop(engine);

        let reopened = Engine::open(dir.path(), config).unwrap();
        let records = WalReader::new(dir.path().join("wal"), reopened.config.wal.clone())
            .scan()
            .unwrap();
        let commit_lsn = records
            .iter()
            .find(|record| {
                record.kind == WalRecordKind::Commit
                    && matches!(
                        WalPayload::decode(&record.payload).unwrap(),
                        WalPayload::Commit { .. }
                    )
            })
            .expect("commit wal record")
            .lsn;
        assert!(
            during <= commit_lsn,
            "checkpoint {during:?} passed unpublished commit {commit_lsn:?}"
        );
        let mut tx = reopened.begin(Isolation::Snapshot).unwrap();
        assert_eq!(reopened.get(&mut tx, row).unwrap(), Some(b"alpha".to_vec()));
    }
}
