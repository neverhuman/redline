//! Engine create/open paths and WAL recovery replay.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::catalog::{
    CatalogManager, CatalogStore, CatalogSyncPolicy, IndexId as CatalogIndexId, bootstrap_schema,
};
use crate::engine::lock::RowLockManager;
use crate::engine::page_heap::PageBackedHeap;
use crate::format::{Lsn, Page, RelId, TxId};
use crate::io::{StdFileSystem, create_dir_all_durable};
use crate::storage::{BufferPool, ControlStore, PageFile, TxStatusStore};
use crate::telemetry::Phase11Counters;
use crate::wal::{
    WalCoordinator, WalPayload, WalReader, WalRecord, WalRecordKind,
    salvage_and_empty_torn_segment, segments_on_disk_with_bytes,
};
use crate::{Error, Result};

use self::timeline::{PlannedFork, ReplayFilter};
use self::wal_floor::wal_resume_position;

use super::{
    CommitDurability, ConcurrentTxStatus, Engine, EngineConfig, RecoveryMetrics, RecoveryOutcome,
    RecoveryReport, RecoveryTarget, TimelineForkReport,
};

impl Engine {
    pub fn create(path: impl AsRef<Path>, config: EngineConfig) -> Result<Arc<Self>> {
        Self::create_inner(path.as_ref(), config, false)
    }

    /// Create a private volatile engine for process-local in-memory database
    /// handles. The engine still uses the regular heap and catalog state
    /// machines, but skips recovery sidecars and WAL writer startup because
    /// there is no durable image to recover.
    pub fn create_volatile(path: impl AsRef<Path>, config: EngineConfig) -> Result<Arc<Self>> {
        Self::create_inner(path.as_ref(), config, true)
    }

    fn create_inner(path: &Path, config: EngineConfig, volatile: bool) -> Result<Arc<Self>> {
        // For volatile (in-memory) databases:
        //   • The caller (EphemeralRoot / OwnedTempRoot) already created the dir,
        //     so skipping create_dir_all saves 3–4 extra syscalls per process.
        //   • The config was built from EngineConfig::default() which deliberately
        //     does NOT call cached_available_parallelism() — avoiding the 4–6
        //     syscall cgroup walk on every fresh process.  Volatile databases use
        //     the small fixed shard counts.
        //
        // For persistent databases: use the config exactly as supplied by the
        // caller.  Callers who need CPU-scaled shards should call
        // `EngineConfig::with_detected_parallelism()` on their config before
        // passing it to Engine::create(); we do NOT override the caller's values
        // here so that explicit test configs (e.g. buffer_pool_pages=16 with
        // heap_lanes=4) are respected as-written.
        if !volatile {
            // A new root is a new name in its parent.
            create_dir_all_durable(&StdFileSystem, path)?;
        }
        let data_path = path.join(&config.data_file_name);
        let wal_dir = path.join("wal");
        let page_file = Arc::new(PageFile::create(&data_path, config.page_size)?);
        if !volatile {
            // The page file name has to be durable before later checkpoints
            // fsync only the file bytes.
            sync_page_file_directory(&data_path)?;
        }
        // W7-perf: for volatile databases use the caller-supplied shard hint so
        // we skip the cgroup walk inside BufferPool::new.  For volatile databases
        // EngineConfig::default() uses fixed baseline values (lock_shards=16,
        // heap_lanes=4), so parallelism_hint = lock_shards / 4 = 4.
        // Persistent databases call BufferPool::new which uses the process-wide
        // OnceLock cache (one cgroup walk total, amortised across all opens).
        let buffer = if volatile {
            let parallelism_hint = (config.lock_shards / 4).max(1);
            let pool = BufferPool::new_with_parallelism(
                page_file,
                config.buffer_pool_pages,
                parallelism_hint,
            )?;
            // Nothing recovers a volatile engine, so its page file is scratch
            // space and any page may be written there to make room.
            pool.use_as_scratch();
            Arc::new(pool)
        } else {
            Arc::new(BufferPool::new(page_file, config.buffer_pool_pages)?)
        };
        let wal = if volatile {
            Arc::new(WalCoordinator::volatile(config.wal.clone()))
        } else {
            Arc::new(WalCoordinator::create_with_shutdown_flush(
                &wal_dir,
                config.wal.clone(),
                flush_wal_on_shutdown(config.commit_durability),
            )?)
        };
        // Use the volatile constructors for in-memory engines: they skip
        // create_dir_all (already done by the caller) saving another 4–6
        // syscalls per process start.  Persistent engines use the regular
        // constructors which also guarantee the subdirectory exists.
        let control = if volatile {
            ControlStore::new_volatile(path)
        } else {
            ControlStore::new(path)?
        };
        let tx_status_store = if volatile {
            TxStatusStore::new_volatile(path)
        } else {
            TxStatusStore::new(path)?
        };
        let checkpoint = if volatile {
            None
        } else {
            control.load_latest()?
        };
        let catalog_store =
            CatalogStore::new_with_sync_policy(path, catalog_sync_policy(config.commit_durability));
        let loaded_catalog = if volatile {
            None
        } else {
            catalog_store.load().ok().flatten()
        };
        let initial_catalog = match loaded_catalog.clone() {
            Some(catalog) => catalog,
            None => bootstrap_schema(RelId(10_000)),
        };
        if !volatile && loaded_catalog.is_none() {
            catalog_store.save_atomic(&initial_catalog)?;
        }
        let buffer = Arc::clone(&buffer);
        let phase11_counters = Arc::new(Phase11Counters::default());
        let locks = Arc::new(RowLockManager::new(config.lock_shards, config.busy_timeout));
        // Wave 1A-F: pipe Phase11 telemetry into the row-lock manager so
        // contended-acquire waits land in `lock_wait_us_buckets`. Same
        // Arc piped into the WAL coordinator so `wal_batch_size_buckets`
        // gets bumped per fdatasync.
        locks.set_phase11_counters(Arc::clone(&phase11_counters));
        wal.set_phase11_counters(Arc::clone(&phase11_counters));
        let heap_wal = if volatile {
            None
        } else {
            Some(Arc::clone(&wal))
        };
        let commit_durability_live = std::sync::atomic::AtomicU8::new(
            commit_durability_initial_u8(config.commit_durability),
        );
        let engine = Arc::new(Self {
            config: config.clone(),
            commit_durability_live,
            volatile,
            data_path,
            wal_dir,
            rel_id: config.rel_id,
            txs: ConcurrentTxStatus::new(),
            buffer: Arc::clone(&buffer),
            heap: PageBackedHeap::new_with_wal(config.rel_id, config.heap_lanes, buffer, heap_wal)?,
            catalog: CatalogManager::new(initial_catalog),
            catalog_store,
            locks,
            wal,
            phase11_counters,
            control,
            tx_status_store,
            checkpoint_serial: std::sync::Mutex::new(()),
            checkpoint: std::sync::Mutex::new(checkpoint),
            index_handles: std::sync::Mutex::new(HashMap::new()),
            recovery_report: std::sync::Mutex::new(None),
        });
        Ok(engine)
    }

    pub fn open(path: impl AsRef<Path>, config: EngineConfig) -> Result<Arc<Self>> {
        Self::open_with_recovery_report(path, config).map(|(engine, _report)| engine)
    }

    pub fn open_with_recovery_report(
        path: impl AsRef<Path>,
        config: EngineConfig,
    ) -> Result<(Arc<Self>, RecoveryReport)> {
        Self::open_with_recovery_report_and_target(path, config, RecoveryTarget::Latest)
    }

    pub fn open_with_recovery_target(
        path: impl AsRef<Path>,
        config: EngineConfig,
        target: RecoveryTarget,
    ) -> Result<Arc<Self>> {
        Self::open_with_recovery_report_and_target(path, config, target).map(|(engine, _)| engine)
    }

    pub fn open_with_recovery_report_and_target(
        path: impl AsRef<Path>,
        config: EngineConfig,
        target: RecoveryTarget,
    ) -> Result<(Arc<Self>, RecoveryReport)> {
        let wal_dir = path.as_ref().join("wal");
        let mut reader = WalReader::new(&wal_dir, config.wal.clone());
        let scan_report = reader.scan_report()?;
        // Which records apply: those below the target, off every timeline
        // an earlier targeted recovery abandoned (workplan R9, step 5).
        let filter = ReplayFilter::new(&scan_report.records, target)?;
        let txs = ConcurrentTxStatus::new();
        create_dir_all_durable(&StdFileSystem, path.as_ref())
            .map_err(|_| Error::CorruptPage("create engine directory failed"))?;
        let control = ControlStore::new(path.as_ref())
            .map_err(|_| Error::CorruptPage("create control store failed"))?;
        let tx_status_store = TxStatusStore::new(path.as_ref())
            .map_err(|_| Error::CorruptPage("create tx status store failed"))?;
        let selection = control.load_selection().map_err(|err| match err {
            // A newer build's control file: say so rather than "corrupt".
            Error::UnsupportedVersion(version) => Error::UnsupportedVersion(version),
            _ => Error::CorruptPage("load control file failed"),
        })?;
        // Everything up to the first write below only reads: the generation,
        // its transaction status, the WAL it needs and the catalog are all
        // checked first, so a check that fails leaves the files as they were
        // (workplan R6, R9).
        let choice = plan::select_recoverable_generation(
            &selection,
            &scan_report,
            &tx_status_store,
            config.wal.segment_bytes,
        )?;
        let checkpoint = choice.checkpoint;
        if let (Some(checkpoint), Some(tx_status)) = (checkpoint, choice.tx_status.as_ref()) {
            // The heap pages already hold every heap record below the heap
            // redo LSN, so no earlier LSN can be recovered to. When the heap
            // is a newer generation's than the status, no CSN target can be
            // checked against it.
            let heap_is_newer = choice.plan.heap_replay_from
                > checkpoint.checkpoint_lsn.max(checkpoint.heap_redo_lsn);
            let too_old = match target {
                RecoveryTarget::Latest => false,
                RecoveryTarget::Lsn(limit) => limit < choice.plan.heap_replay_from,
                RecoveryTarget::Csn(limit) => heap_is_newer || limit < tx_status.published_csn,
            };
            if too_old {
                return Err(Error::CorruptWal(
                    "requested recovery target is older than checkpoint base",
                ));
            }
        }
        let page_path = path.as_ref().join(&config.data_file_name);
        let existing_page_file = if checkpoint.is_some() || page_path.exists() {
            Some(
                PageFile::open(&page_path, config.page_size)
                    .map_err(|_| Error::CorruptPage("open recovered page file failed"))?,
            )
        } else {
            None
        };
        let holds_data = checkpoint.is_some()
            || existing_page_file
                .as_ref()
                .map(|page_file| page_file.page_count())
                .transpose()?
                .is_some_and(|pages| pages > 0);
        let catalog_store = CatalogStore::new_with_sync_policy(
            path.as_ref(),
            catalog_sync_policy(config.commit_durability),
        );
        let catalog_snapshot = plan::resolve_catalog(
            catalog_store.load(),
            recover_catalog_snapshot(&scan_report.records, &filter)?,
            holds_data,
        )?;
        // Never append below where recovery replays from, heap redo included,
        // nor below a page LSN, even when the WAL that reached them is gone.
        let resume = wal_resume_position(
            &scan_report,
            choice.plan.heap_replay_from,
            &segments_on_disk_with_bytes(&wal_dir)?,
            config.wal.segment_bytes,
            || {
                existing_page_file
                    .as_ref()
                    .map_or(Ok(Lsn::ZERO), |page_file| page_file.max_page_lsn())
            },
        )?;
        // With no checkpoint the WAL must run from LSN 0. A page past that
        // with no record left behind it means the log that wrote it is gone.
        if checkpoint.is_none()
            && scan_report.records.is_empty()
            && resume.summary.valid_end_lsn > Lsn::ZERO
        {
            return Err(Error::CorruptWal(
                "no checkpoint covers pages the wal no longer holds",
            ));
        }
        let page_file = match existing_page_file {
            Some(page_file) => Arc::new(page_file),
            None => {
                let created = PageFile::create(&page_path, config.page_size)
                    .map_err(|_| Error::CorruptPage("create recovered page file failed"))?;
                sync_page_file_directory(&page_path)
                    .map_err(|_| Error::CorruptPage("sync page file directory failed"))?;
                Arc::new(created)
            }
        };
        // The one change to existing WAL bytes before replay: a torn segment
        // below where the log resumes would fail the next scan once the
        // resumed segment exists. Its bytes are kept under `wal/salvage/`
        // first. Any other torn tail stays in place until recovery succeeds.
        let mut salvage = Vec::new();
        if let Some(segment) = resume.torn_segment_to_empty {
            salvage.push(salvage_and_empty_torn_segment(&wal_dir, segment)?);
        }
        let wal_open_summary = resume.summary;
        let buffer = Arc::new(
            BufferPool::new(Arc::clone(&page_file), config.buffer_pool_pages)
                .map_err(|_| Error::CorruptPage("create buffer pool failed"))?,
        );
        let wal = Arc::new(
            WalCoordinator::open_with_scan_summary_and_shutdown_flush(
                &wal_dir,
                config.wal.clone(),
                wal_open_summary,
                flush_wal_on_shutdown(config.commit_durability),
            )
            .map_err(|_| Error::CorruptWal("open wal coordinator failed"))?,
        );
        // Heap replay stamps pages with LSN zero, which eviction may write,
        // and page images go straight to the page file, so no page LSN says
        // which WAL a replayed change needs. The scanned WAL can still sit
        // unsynced from the run that crashed. Make it durable before replay
        // writes any page.
        wal.flush_all()?;
        let heap = PageBackedHeap::new_with_wal(
            config.rel_id,
            config.heap_lanes,
            Arc::clone(&buffer),
            Some(Arc::clone(&wal)),
        )
        .map_err(|_| Error::CorruptPage("create heap failed"))?;
        if let Some(tx_status) = choice.tx_status {
            for (tx_id, csn) in tx_status.entries {
                txs.publish_recovered_commit(tx_id, csn);
            }
            txs.restore_frontier(
                tx_status.next_tx,
                tx_status.next_csn,
                tx_status.published_csn,
            );
        }
        // LSN sentinel: with no checkpoint, recovery replays the entire WAL
        // starting from the very beginning.
        let replay_from_lsn = choice.plan.replay_from_lsn;
        // The checkpoint wrote heap pages holding every heap record below its
        // heap redo LSN and none above it.
        let heap_replay_from = choice.plan.heap_replay_from;
        // Heap redo writes each row once; the page file must not already
        // hold a copy that an earlier, interrupted recovery wrote. A
        // version-1 checkpoint (v4.1.0) may have left committed rows it
        // already covered on pages past its page count, so those pages stay,
        // with the duplicate-row risk that build already had.
        if choice.plan.clear_heap_past_count {
            replayed_pages::clear_heap_pages_past_checkpoint(
                &page_file,
                &buffer,
                &heap,
                config.rel_id,
                choice.plan.heap_page_count,
            )?;
        }
        reserve_ids_named_in_wal(&scan_report.records, &txs, &heap)?;
        recover_index_page_images(&scan_report.records, replay_from_lsn, &filter, &buffer)?;
        let metrics = recover_heap(&scan_report.records, heap_replay_from, &filter, &txs, &heap)?;
        // Every commit recovery will publish is published now. A CSN below
        // them that the WAL lacks was given up before the crash.
        txs.seal_recovered_frontier();
        // A crash here leaves in the page file whatever replayed heap pages
        // eviction wrote, with no checkpoint that covers them.
        crate::fail_point!("engine::recovery::after_heap_replay");
        #[cfg(test)]
        replayed_pages::run_after_heap_replay_hook()?;
        let catalog = CatalogManager::new(catalog_snapshot);
        let phase11_counters = Arc::new(Phase11Counters::default());
        let locks = Arc::new(RowLockManager::new(config.lock_shards, config.busy_timeout));
        // Wave 1A-F: same telemetry pipe on the open path.
        locks.set_phase11_counters(Arc::clone(&phase11_counters));
        wal.set_phase11_counters(Arc::clone(&phase11_counters));
        let commit_durability_live = std::sync::atomic::AtomicU8::new(
            commit_durability_initial_u8(config.commit_durability),
        );
        let engine = Arc::new(Self {
            config: config.clone(),
            commit_durability_live,
            volatile: false,
            data_path: page_path,
            wal_dir,
            rel_id: config.rel_id,
            txs,
            buffer: Arc::clone(&buffer),
            heap,
            catalog,
            catalog_store,
            locks,
            wal,
            phase11_counters,
            control,
            tx_status_store,
            checkpoint_serial: std::sync::Mutex::new(()),
            checkpoint: std::sync::Mutex::new(checkpoint),
            index_handles: std::sync::Mutex::new(HashMap::new()),
            recovery_report: std::sync::Mutex::new(None),
        });
        engine.rehydrate_index_handles()?;
        recover_indexes(&scan_report.records, replay_from_lsn, &filter, &engine)?;
        if checkpoint.is_some() {
            let page_count = engine.heap.page_count()?;
            engine
                .heap
                .load_row_directory_from_pages(page_count, &engine.txs)?;
            engine.heap.load_reusable_pages_from_pages(page_count)?;
        }
        // Recovery has succeeded. Only now is a torn tail copied to
        // `wal/salvage/` and cut from the log (workplan R9, step 3).
        salvage.extend(engine.wal.settle_torn_tail()?);
        let timeline_fork = match filter.fork_to_record(&scan_report.records) {
            Some(planned) => Some(record_timeline_fork(&engine, planned)?),
            None => None,
        };
        // Replay puts heap rows into new versions on new pages, and eviction
        // may write those pages. The next recovery would replay the same
        // records again next to the copies already in the file, so once
        // replay has written a page, checkpoint past it. After a targeted
        // recovery the fork record is durable by now, so the next open
        // cannot publish a commit past the target from below the checkpoint.
        if engine.buffer.stats().writes > 0 {
            engine.checkpoint_with_stats()?;
        }
        let report = RecoveryReport::from_scan(
            &scan_report,
            config.wal.segment_bytes,
            RecoveryOutcome {
                metrics,
                replay_from_lsn,
                target,
                checkpoint_generation: checkpoint.map(|control| control.generation),
                skipped_generation: choice.skipped_generation,
                warnings: choice.warnings,
                salvage,
                timeline_fork,
                abandoned_wal: filter.abandoned().to_vec(),
            },
        );
        *engine
            .recovery_report
            .lock()
            .map_err(|_| Error::CorruptPage("engine recovery report mutex poisoned"))? =
            Some(report.clone());
        Ok((engine, report))
    }
}

/// Append and flush the fork record that keeps later recoveries at this
/// one's target.
fn record_timeline_fork(engine: &Engine, planned: PlannedFork) -> Result<TimelineForkReport> {
    let append = engine.wal.append(
        WalRecordKind::TimelineFork,
        TxId::ZERO,
        planned.payload().encode()?,
    )?;
    engine.wal.flush_until(append.end_lsn)?;
    Ok(TimelineForkReport {
        fork_lsn: planned.fork_lsn,
        record_lsn: append.start_lsn,
        parent: planned.parent,
        child: planned.child,
    })
}

pub(super) fn catalog_sync_policy(durability: CommitDurability) -> CatalogSyncPolicy {
    match durability {
        // Normal and UnsafeDev write the schema file and skip fsync,
        // matching the WAL commit policy. Only Strict fsyncs it.
        CommitDurability::Strict => CatalogSyncPolicy::Durable,
        CommitDurability::Normal | CommitDurability::UnsafeDev => CatalogSyncPolicy::Volatile,
    }
}

fn flush_wal_on_shutdown(commit_durability: CommitDurability) -> bool {
    !matches!(commit_durability, CommitDurability::UnsafeDev)
}

/// Encode the open-time `CommitDurability` to the u8 representation used by
/// `Engine::commit_durability_live`. Kept private to the kernel so the
/// `Engine::commit_durability` / `set_commit_durability` accessors are the
/// only public surface.
fn commit_durability_initial_u8(durability: CommitDurability) -> u8 {
    match durability {
        CommitDurability::Strict => 0,
        CommitDurability::Normal => 1,
        CommitDurability::UnsafeDev => 2,
    }
}

/// Move the next transaction id past every id the scanned WAL names, the
/// next row id past every heap row id it names, and the next CSN past every
/// commit record's CSN, committed or not.
///
/// Rollback and abandonment log nothing, so a rolled-back transaction's
/// records stay in the WAL under its id. If a transaction after the reopen
/// took that id and committed, even with no writes, its commit record would
/// make the next recovery replay the old records as committed. Eviction can
/// also write a page holding tuples of a transaction that never committed,
/// and a checkpoint need not have recorded that transaction's id. A page
/// reaches the file only after the WAL records behind it are durable, so the
/// scanned WAL names every such id. A new row could likewise take a row id
/// the orphaned tuples still carry.
///
/// This walks every record, whatever the replay start and recovery target:
/// an id is spent once it is logged, even where replay does not apply it. A
/// commit past a recovery target, or on a timeline a fork abandoned, keeps
/// its CSN, so a CSN names at most one commit in the log.
fn reserve_ids_named_in_wal(
    records: &[WalRecord],
    txs: &ConcurrentTxStatus,
    heap: &PageBackedHeap,
) -> Result<()> {
    for record in records {
        if !matches!(
            record.kind,
            WalRecordKind::PageDelta | WalRecordKind::Commit
        ) {
            txs.advance_next_tx_past(record.tx_id)?;
            continue;
        }
        // Replay trusts the transaction id inside these payloads, not the
        // header's, so reserve that one as well.
        let payload = WalPayload::decode(&record.payload)?;
        // `BtreeIndex::delete_mark` logs its delete under the
        // non-transactional marker, the last id. It names no transaction.
        let marker = crate::index::NON_TRANSACTIONAL_DELETE_TX;
        let marker_delete = record.tx_id == marker
            && matches!(payload, WalPayload::IndexDelete { tx_id, .. } if tx_id == marker);
        if !marker_delete {
            txs.advance_next_tx_past(record.tx_id)?;
            txs.advance_next_tx_past(payload.tx_id())?;
        }
        match payload {
            WalPayload::HeapInsert { row_id, .. }
            | WalPayload::HeapUpdate { row_id, .. }
            | WalPayload::HeapDelete { row_id, .. } => heap.reserve_recovered_row_id(row_id),
            WalPayload::Commit { csn, .. } => txs.advance_next_csn_past(csn),
            _ => {}
        }
    }
    Ok(())
}

fn recover_heap(
    records: &[WalRecord],
    replay_from_lsn: Lsn,
    filter: &ReplayFilter,
    txs: &ConcurrentTxStatus,
    heap: &PageBackedHeap,
) -> Result<RecoveryMetrics> {
    let mut committed = HashMap::new();
    let mut metrics = RecoveryMetrics::default();
    for record in records {
        if record.kind == WalRecordKind::Commit {
            match WalPayload::decode(&record.payload)? {
                WalPayload::Commit { tx_id, csn } => {
                    if filter.commit_visible(record.lsn, csn) {
                        committed.insert(tx_id, csn);
                        txs.publish_recovered_commit(tx_id, csn);
                        metrics.commits_recovered += 1;
                    }
                }
                _ => return Err(Error::CorruptWal("commit record has non-commit payload")),
            }
        }
    }

    for record in records {
        if record.lsn < replay_from_lsn || !filter.applies(record.lsn) {
            continue;
        }
        if record.kind == WalRecordKind::Commit {
            continue;
        }
        match WalPayload::decode(&record.payload)? {
            WalPayload::PageImage {
                page_id: _,
                page_lsn: _,
                page_bytes,
            } if committed.contains_key(&record.tx_id) => {
                let page = Page::from_bytes(page_bytes)?;
                if page.header()?.kind == crate::format::PageKind::Heap {
                    heap.redo_page_image(page, record_end_lsn(record))?;
                    metrics.page_images_redone += 1;
                }
            }
            WalPayload::PageImage { .. } => {}
            WalPayload::HeapInsert {
                tx_id,
                rel_id,
                row_id,
                payload,
            } if record.kind == WalRecordKind::PageDelta && committed.contains_key(&tx_id) => {
                heap.insert_recovered_for_relation(tx_id, rel_id, row_id, payload)?;
                metrics.legacy_mutations_redone += 1;
            }
            WalPayload::HeapUpdate {
                tx_id,
                rel_id,
                row_id,
                payload,
            } if record.kind == WalRecordKind::PageDelta && committed.contains_key(&tx_id) => {
                heap.update_recovered_for_relation(tx_id, rel_id, row_id, payload)?;
                metrics.legacy_mutations_redone += 1;
            }
            WalPayload::HeapDelete {
                tx_id,
                rel_id,
                row_id,
            } if record.kind == WalRecordKind::PageDelta && committed.contains_key(&tx_id) => {
                heap.delete_recovered_for_relation(tx_id, rel_id, row_id)?;
                metrics.legacy_mutations_redone += 1;
            }
            WalPayload::HeapInsert { .. }
            | WalPayload::HeapUpdate { .. }
            | WalPayload::HeapDelete { .. } => {}
            WalPayload::IndexInsert { .. } | WalPayload::IndexDelete { .. } => {}
            WalPayload::SegmentSeal { .. }
            | WalPayload::BackupBegin { .. }
            | WalPayload::BackupEnd { .. }
            | WalPayload::TimelineFork { .. }
            | WalPayload::LogicalTxn { .. }
            | WalPayload::CatalogSnapshot { .. } => {}
            // WS-A6 multi-writer hot-row: the coordinator emits this
            // record alongside the per-batch HeapUpdate, so recovery's
            // heap-state reconstruction comes from the HeapUpdate path
            // above. The CombinedSemanticDelta serves as an audit /
            // observability marker (batched_count = how many original
            // UPDATEs the coordinator merged); we accept and decode it
            // here for forward-compat but do not re-apply.
            WalPayload::CombinedSemanticDelta { .. } => {}
            WalPayload::Commit { .. } => unreachable!("commit records are skipped above"),
        }
    }

    Ok(metrics)
}

/// Redo every B-tree page image at or after the checkpoint, in LSN order,
/// over a page that does not already hold it.
///
/// A split's images describe structure, not the logging transaction's data:
/// the split stays in memory when that transaction rolls back, and each
/// entry in an image carries its own create and delete transaction, which
/// visibility checks. A checkpoint taken mid-split stops at the split's
/// first image and does not write the split's pages, whose page LSNs are
/// past it. Those images are then the only copy of the committed entries
/// the split moved, so they replay whether or not the transaction that
/// logged them committed. A split appends its new right page's image before
/// the image of the page that links to it, so a replayed link never names a
/// page whose image is missing.
///
/// An image replaces the page only when the page's LSN, resident or in the
/// file, is below the end of the image's record. A page at or past it holds
/// that image and later changes, which an older image would drop. The
/// reopened WAL resumes past every page LSN, so a page LSN never comes from
/// an earlier run of the log.
fn recover_index_page_images(
    records: &[WalRecord],
    replay_from_lsn: Lsn,
    filter: &ReplayFilter,
    buffer: &Arc<BufferPool>,
) -> Result<()> {
    for record in records {
        if record.lsn < replay_from_lsn || !filter.applies(record.lsn) {
            continue;
        }
        if record.kind == WalRecordKind::Commit {
            continue;
        }
        if let WalPayload::PageImage {
            page_id: _,
            page_lsn: _,
            page_bytes,
        } = WalPayload::decode(&record.payload)?
        {
            let page = Page::from_bytes(page_bytes)?;
            match page.header()?.kind {
                crate::format::PageKind::BtreeMeta
                | crate::format::PageKind::BtreeLeaf
                | crate::format::PageKind::BtreeInternal => {
                    buffer.redo_page_image(page, record_end_lsn(record))?;
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn recover_catalog_snapshot(
    records: &[WalRecord],
    filter: &ReplayFilter,
) -> Result<Option<Arc<crate::catalog::SchemaSnapshot>>> {
    let mut committed = HashSet::new();
    for record in records {
        if record.kind == WalRecordKind::Commit
            && let WalPayload::Commit { tx_id, csn } = WalPayload::decode(&record.payload)?
            && filter.commit_visible(record.lsn, csn)
        {
            committed.insert(tx_id);
        }
    }

    let mut latest: Option<(Lsn, Arc<crate::catalog::SchemaSnapshot>)> = None;
    for record in records {
        if record.kind != WalRecordKind::Logical
            || !committed.contains(&record.tx_id)
            || !filter.applies(record.lsn)
        {
            continue;
        }
        let WalPayload::CatalogSnapshot {
            tx_id: _,
            schema_epoch: _,
            snapshot,
        } = WalPayload::decode(&record.payload)?
        else {
            continue;
        };
        let snapshot = Arc::new(crate::catalog::decode_snapshot(&snapshot)?);
        match &latest {
            Some((lsn, _)) if *lsn >= record.lsn => {}
            _ => latest = Some((record.lsn, snapshot)),
        }
    }

    Ok(latest.map(|(_, snapshot)| snapshot))
}

fn recover_indexes(
    records: &[WalRecord],
    replay_from_lsn: Lsn,
    filter: &ReplayFilter,
    engine: &Arc<Engine>,
) -> Result<()> {
    let mut committed = HashSet::new();
    for record in records {
        if record.kind == WalRecordKind::Commit
            && let WalPayload::Commit { tx_id, csn } = WalPayload::decode(&record.payload)?
            && filter.commit_visible(record.lsn, csn)
        {
            committed.insert(tx_id);
        }
    }

    for record in records {
        if record.lsn < replay_from_lsn || !filter.applies(record.lsn) {
            continue;
        }
        if record.kind == WalRecordKind::Commit {
            continue;
        }
        match WalPayload::decode(&record.payload)? {
            // `recover_index_page_images` already installed every committed
            // index image, in LSN order, through the buffer pool. Writing an
            // image again here would go straight to the page file: once the
            // pool has evicted that page, an older image replaces the newer
            // one, and the deltas below cannot restore what it drops.
            WalPayload::PageImage { .. } => {}
            WalPayload::IndexInsert {
                tx_id,
                index_id,
                logical_key,
                row,
            } if record.kind == WalRecordKind::PageDelta && committed.contains(&tx_id) => {
                let handles = engine
                    .index_handles
                    .lock()
                    .map_err(|_| Error::CorruptPage("engine index handles mutex poisoned"))?;
                if let Some(handle) = handles.get(&CatalogIndexId(index_id)) {
                    handle.insert_recovered_tx(tx_id, &logical_key, row, record_end_lsn(record))?;
                }
            }
            WalPayload::IndexDelete {
                tx_id,
                index_id,
                logical_key,
                row,
            } if record.kind == WalRecordKind::PageDelta && committed.contains(&tx_id) => {
                let handles = engine
                    .index_handles
                    .lock()
                    .map_err(|_| Error::CorruptPage("engine index handles mutex poisoned"))?;
                if let Some(handle) = handles.get(&CatalogIndexId(index_id)) {
                    handle.delete_mark_recovered_tx(
                        tx_id,
                        &logical_key,
                        row,
                        record_end_lsn(record),
                    )?;
                }
            }
            WalPayload::HeapInsert { .. }
            | WalPayload::HeapUpdate { .. }
            | WalPayload::HeapDelete { .. }
            | WalPayload::IndexInsert { .. }
            | WalPayload::IndexDelete { .. }
            | WalPayload::Commit { .. }
            | WalPayload::SegmentSeal { .. }
            | WalPayload::BackupBegin { .. }
            | WalPayload::BackupEnd { .. }
            | WalPayload::TimelineFork { .. }
            | WalPayload::LogicalTxn { .. }
            | WalPayload::CatalogSnapshot { .. }
            | WalPayload::CombinedSemanticDelta { .. } => {}
        }
    }
    Ok(())
}

fn record_end_lsn(record: &WalRecord) -> Lsn {
    Lsn(record.lsn.0 + record.encoded_len() as u64)
}

fn sync_page_file_directory(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    crate::storage::sync_parent_dir(parent)
}

#[path = "recovery_wal_floor.rs"]
mod wal_floor;

#[path = "recovery_plan.rs"]
mod plan;

#[path = "recovery_timeline.rs"]
mod timeline;

#[path = "recovery_replayed_pages.rs"]
mod replayed_pages;

#[cfg(test)]
#[path = "recovery_replay_crash_tests.rs"]
mod replay_crash_tests;

#[cfg(test)]
#[path = "recovery_dir_sync_tests.rs"]
mod dir_sync_tests;

#[cfg(test)]
#[path = "recovery_index_image_tests.rs"]
mod index_image_tests;

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::{Engine, EngineConfig};
    use crate::storage::take_parent_dir_syncs;

    fn small_config() -> EngineConfig {
        EngineConfig {
            buffer_pool_pages: 16,
            ..EngineConfig::default()
        }
    }

    #[test]
    fn persistent_create_syncs_the_page_file_directory() {
        let dir = TempDir::new().unwrap();
        let _ = take_parent_dir_syncs();
        let engine = Engine::create(dir.path(), small_config()).unwrap();
        assert_eq!(take_parent_dir_syncs(), 1);
        drop(engine);
    }

    #[test]
    fn volatile_create_does_not_sync_the_page_file_directory() {
        let dir = TempDir::new().unwrap();
        let _ = take_parent_dir_syncs();
        let engine = Engine::create_volatile(dir.path(), small_config()).unwrap();
        assert_eq!(take_parent_dir_syncs(), 0);
        drop(engine);
    }
}
