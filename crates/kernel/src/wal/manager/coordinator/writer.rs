use std::sync::Arc;

use crate::format::Lsn;
use crate::wal::policy::{ActiveWalSchedulePolicy, WalScheduleContext, WalSchedulePolicy};

use super::helpers::{
    bump_phase11_wal_batch, drain_until, publish_wal_failure, publish_written_lsn,
    resample_flush_target, wait_for_group_commit_window,
};
use super::*;

pub(super) fn wal_writer_loop(
    mut wal: WalManager,
    config: WalConfig,
    shared: Arc<WalCoordinatorShared>,
    flush_on_shutdown: bool,
) {
    // Lane GC: track records and bytes that have been written but
    // not yet fsynced, so the histogram bump after `wal.flush()`
    // captures the *exact* batch size covered by that one syscall.
    // Reset on every successful group fsync.
    let mut group_records: u64 = 0;
    let mut group_bytes: u64 = 0;
    loop {
        let mut batch = Vec::new();
        let mut flush_target = Lsn::ZERO;
        let mut should_flush = false;
        let shutdown;
        let settle_tail;

        {
            let mut state = match shared.state.lock() {
                Ok(state) => state,
                Err(_) => return,
            };

            while !state.write_requested
                && state.flush_requested_lsn <= state.durable_lsn
                && !state.shutdown
                && !matches!(state.tail_settle, TailSettle::Requested)
            {
                state = match shared.cvar.wait(state) {
                    Ok(state) => state,
                    Err(_) => return,
                };
            }

            if state.shutdown && state.pending.is_empty() {
                // Written bytes can sit past durable_lsn with nothing left queued.
                // Sync them on the way out unless shutdown flush is disabled.
                let needs_sync = flush_on_shutdown && state.written_lsn > state.durable_lsn;
                drop(state);
                if needs_sync
                    && !sync_written_on_shutdown(
                        &mut wal,
                        &shared,
                        &mut group_records,
                        &mut group_bytes,
                    )
                {
                    return;
                }
                return;
            }

            let batch_limit = ActiveWalSchedulePolicy::write_batch_bytes(
                WalScheduleContext::with_pending(&config, state.pending_bytes, state.pending.len()),
            );
            state.write_requested = false;
            let mut batch_bytes = 0_usize;
            while let Some(record) = state.pending.pop_front() {
                batch_bytes += record.encoded.len();
                state.pending_bytes = state.pending_bytes.saturating_sub(record.encoded.len());
                batch.push(record);
                if batch_bytes >= batch_limit {
                    break;
                }
            }
            if !state.pending.is_empty() {
                state.write_requested = true;
            }

            if state.flush_requested_lsn > state.durable_lsn {
                flush_target = state.flush_requested_lsn;
                should_flush = true;
            }
            shutdown = state.shutdown;
            settle_tail = matches!(state.tail_settle, TailSettle::Requested);
            shared.cvar.notify_all();
        }

        if settle_tail {
            // Recovery has succeeded and asks for the torn tail to be kept
            // and cut off now, before anything else can scan the log.
            if let Err(err) = wal.settle_torn_tail() {
                publish_wal_failure(
                    &shared,
                    WalFailure::new(WalFailureStage::Write, &err, wal.written_lsn()),
                );
                return;
            }
            let Ok(mut state) = shared.state.lock() else {
                return;
            };
            state.tail_settle = TailSettle::Done(wal.salvaged.clone());
            shared.cvar.notify_all();
        }

        let mut last_written = Lsn::ZERO;
        for record in &batch {
            last_written = record.append.end_lsn;
            // Lane GC: accumulate before the write so a torn-write
            // failpoint doesn't desync the counter from durable
            // state — the failure path returns immediately.
            group_records = group_records.saturating_add(1);
            group_bytes = group_bytes.saturating_add(record.encoded.len() as u64);
        }
        if let Err((stage, err)) = wal.write_encoded_batch_staged(&batch) {
            publish_wal_failure(&shared, WalFailure::new(stage, &err, wal.written_lsn()));
            return;
        }
        if last_written != Lsn::ZERO {
            publish_written_lsn(&shared, last_written);
        }

        if should_flush {
            wait_for_group_commit_window(&shared, &config, &mut wal, flush_target);
            // Wave 1A-F: re-sample `flush_requested_lsn` after the
            // group-commit window. Late-arriving commits within the
            // same fdatasync interval are now folded into this train
            // so they don't have to wait for the next sync. The
            // widening MUST happen before `wal.flush()` so durability
            // is preserved: every commit whose LSN <= the post-resample
            // target lands on disk before the corresponding writer is
            // told the commit succeeded. We do not extend the window —
            // just one re-sample, then sync.
            flush_target = resample_flush_target(&shared, &config, flush_target);
            // Lane GC: drain_until may pop & write further records
            // that share this fsync; it returns the count and bytes
            // it wrote so we attribute them to the same group.
            match drain_until(
                &shared,
                &mut wal,
                flush_target,
                ActiveWalSchedulePolicy::drain_batch_bytes(WalScheduleContext::from_config(
                    &config,
                )),
            ) {
                Ok(drained) => {
                    group_records = group_records.saturating_add(drained.records);
                    group_bytes = group_bytes.saturating_add(drained.bytes);
                }
                Err(failure) => {
                    publish_wal_failure(&shared, failure);
                    return;
                }
            }
            match wal.flush() {
                Ok(durable_lsn) => {
                    // Lane GC: bump group-commit telemetry on the
                    // shared counters before resetting locals. Skip
                    // empty drains (latency-only flushes) so the
                    // mean-fan-in stat stays meaningful.
                    if let Some(counters) = wal.sync_counters.as_ref()
                        && group_records > 0
                    {
                        counters.record_group_commit(group_records, group_bytes);
                    }
                    // Wave 1A-F: bump Phase 11 wal_batch_size_buckets
                    // with the per-fdatasync record count. Same shape
                    // as `group_commit_batch_buckets` so the paper
                    // figs can reuse the existing bucketing.
                    bump_phase11_wal_batch(&shared, group_records);
                    group_records = 0;
                    group_bytes = 0;
                    if let Ok(mut state) = shared.state.lock() {
                        state.durable_lsn = durable_lsn;
                        if state.flush_requested_lsn <= durable_lsn {
                            state.flush_requested_lsn = Lsn::ZERO;
                        }
                        shared.cvar.notify_all();
                    } else {
                        return;
                    }
                }
                Err(err) => {
                    publish_wal_failure(
                        &shared,
                        WalFailure::new(WalFailureStage::Flush, &err, wal.durable_lsn()),
                    );
                    return;
                }
            }
        } else if shutdown
            && flush_on_shutdown
            && last_written != Lsn::ZERO
            && !sync_written_on_shutdown(&mut wal, &shared, &mut group_records, &mut group_bytes)
        {
            return;
        }
    }
}

fn sync_written_on_shutdown(
    wal: &mut WalManager,
    shared: &Arc<WalCoordinatorShared>,
    group_records: &mut u64,
    group_bytes: &mut u64,
) -> bool {
    match wal.flush() {
        Ok(durable_lsn) => {
            if let Some(counters) = wal.sync_counters.as_ref()
                && *group_records > 0
            {
                counters.record_group_commit(*group_records, *group_bytes);
            }
            bump_phase11_wal_batch(shared, *group_records);
            *group_records = 0;
            *group_bytes = 0;
            if let Ok(mut state) = shared.state.lock() {
                state.durable_lsn = durable_lsn;
                shared.cvar.notify_all();
                true
            } else {
                false
            }
        }
        Err(err) => {
            publish_wal_failure(
                shared,
                WalFailure::new(WalFailureStage::Flush, &err, wal.durable_lsn()),
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tempfile::TempDir;

    use crate::format::TxId;
    use crate::wal::{WalConfig, WalCoordinator, WalRecordKind};

    fn config() -> WalConfig {
        WalConfig {
            segment_bytes: 1024 * 1024,
            group_commit_delay_us: 0,
            ..WalConfig::default()
        }
    }

    fn write_without_flush(flush_on_shutdown: bool) -> (TempDir, WalCoordinator) {
        let dir = TempDir::new().unwrap();
        let coord =
            WalCoordinator::create_with_shutdown_flush(dir.path(), config(), flush_on_shutdown)
                .unwrap();
        let append = coord
            .append(WalRecordKind::Begin, TxId(1), b"begin".to_vec())
            .unwrap();
        let written = coord.write_until(append.end_lsn).unwrap();
        assert!(written >= append.end_lsn);
        assert!(coord.durable_lsn().unwrap() < append.end_lsn);
        assert_eq!(coord.sync_counters_snapshot().fdatasyncs_issued, 0);
        (dir, coord)
    }

    #[test]
    fn shutdown_fsyncs_records_already_written() {
        let (dir, coord) = write_without_flush(true);
        let counters = Arc::clone(&coord.sync_counters);
        drop(coord);
        let snap = counters.snapshot();
        assert_eq!(snap.fdatasyncs_issued, 1);
        assert_eq!(snap.group_commits_issued, 1);
        drop(dir);
    }

    #[test]
    fn shutdown_skips_fsync_when_disabled() {
        let (dir, coord) = write_without_flush(false);
        let counters = Arc::clone(&coord.sync_counters);
        drop(coord);
        assert_eq!(counters.snapshot().fdatasyncs_issued, 0);
        drop(dir);
    }

    #[test]
    fn shutdown_does_not_fsync_again_when_already_durable() {
        let dir = TempDir::new().unwrap();
        let coord = WalCoordinator::create(dir.path(), config()).unwrap();
        let append = coord
            .append(WalRecordKind::Begin, TxId(1), b"begin".to_vec())
            .unwrap();
        coord.flush_until(append.end_lsn).unwrap();
        let before = coord.sync_counters_snapshot().fdatasyncs_issued;
        assert_eq!(before, 1);
        let counters = Arc::clone(&coord.sync_counters);
        drop(coord);
        assert_eq!(counters.snapshot().fdatasyncs_issued, before);
        drop(dir);
    }
}
