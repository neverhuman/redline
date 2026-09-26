//! Inspection, checkpoint, statistics, vacuum, and integrity entrypoints.

use std::collections::HashMap;
use std::sync::Arc;

use crate::catalog::IndexId as CatalogIndexId;
use crate::engine::page_heap::{HeapScanRow, PageBackedHeap, ParallelScanDiagnostics, VacuumStats};
use crate::format::{Csn, PageId, RelId, RowId, TuplePtr, TxId};
use crate::index::BtreeIndex;
use crate::storage::{
    BufferPool, BufferPoolStats, ControlFile, DEFAULT_CHECKPOINT_BATCH_PAGES, PageFile,
    TxStatusCheckpoint,
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

    pub fn checkpoint_with_stats(&self) -> Result<CheckpointStats> {
        if self.volatile {
            return Ok(CheckpointStats {
                control: ControlFile::default(),
                flushed_pages: 0,
                flush_batches: 0,
            });
        }
        let durable_lsn = self.wal.flush_all()?;
        // Do not record a checkpoint past a WAL record whose page image is
        // still unpublished. Recovery would skip that record.
        let checkpoint_lsn = self.wal.checkpoint_horizon(durable_lsn)?;
        self.buffer.note_evict_durable_lsn(durable_lsn);
        let flush = self
            .heap
            .flush_dirty_batches(checkpoint_lsn, DEFAULT_CHECKPOINT_BATCH_PAGES)?;
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
        self.catalog_store
            .save_atomic(self.catalog.current().as_ref())?;
        // Lane E failpoint: armed before the new control-file generation lands
        // on disk. A crash here forces recovery to fall back to the previous
        // generation, exercising the dual-control-file protocol.
        crate::fail_point!("engine::checkpoint");
        let next = self
            .control
            .write_next(*checkpoint, checkpoint_lsn, page_count)?;
        self.wal
            .prune_segments_below_checkpoint_lsn(next.checkpoint_lsn)?;
        *checkpoint = Some(next);
        Ok(CheckpointStats {
            control: next,
            flushed_pages: flush.flushed_pages,
            flush_batches: flush.batches,
        })
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
