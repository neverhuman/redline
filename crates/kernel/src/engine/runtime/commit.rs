use std::time::Instant;

use crate::engine::tx::ReservedCsn;
use crate::wal::manager::PageInstallFence;

use super::*;

#[cfg(feature = "failpoints")]
std::thread_local! {
    /// Lane KH P0 #3: per-thread switch for the
    /// `engine::commit::before_publish` failpoint. The fail-crate
    /// registry is process-wide, so without this guard the closure
    /// would inject the fault on every commit in every parallel test.
    /// Tests call [`arm_commit_failure_for_thread`] before issuing
    /// their commit and clear it afterwards.
    static COMMIT_FAILURE_ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Arm or disarm the thread-local commit-failure injection used by the
/// `engine::commit::before_publish` failpoint closure. Available only
/// when the kernel is built with `failpoints` so production builds pay
/// nothing for it.
#[cfg(feature = "failpoints")]
pub fn arm_commit_failure_for_thread(armed: bool) {
    COMMIT_FAILURE_ARMED.with(|c| c.set(armed));
}

#[cfg(feature = "failpoints")]
fn commit_failure_armed_for_thread() -> bool {
    COMMIT_FAILURE_ARMED.with(|c| c.get())
}

#[cfg(not(feature = "failpoints"))]
#[allow(dead_code)]
fn commit_failure_armed_for_thread() -> bool {
    false
}

/// Lane E failpoint: the WAL barrier passed but the CSN is not yet visible
/// to in-memory observers. `panic` fails the commit there. `return` makes
/// a commit on a thread armed with `arm_commit_failure_for_thread` report
/// `MaybeCommitted` after publishing it locally, so higher layers can
/// surface the uncertainty without replaying any SQL-side index repair.
fn injected_commit_outcome(csn: Csn) -> CommitOutcome {
    crate::fail_point!("engine::commit::before_publish", |_detail: Option<
        String,
    >| {
        if commit_failure_armed_for_thread() {
            CommitOutcome::MaybeCommitted
        } else {
            CommitOutcome::Committed(csn)
        }
    });
    CommitOutcome::Committed(csn)
}

/// How long a commit waits for earlier CSNs to publish before it reports the
/// stall on stderr. It keeps waiting afterwards.
const PUBLISH_WAIT_WARNING: Duration = Duration::from_secs(5);

impl Engine {
    /// Commit `tx`.
    ///
    /// - `Ok(Committed(csn))`: the commit passed the durability barrier of
    ///   the live mode and is visible to every snapshot taken after this
    ///   returns.
    /// - `Err(CommitOutcomeUnknown)`: the commit record was queued and the
    ///   WAL failed before it was known written (Normal) or durable
    ///   (Strict). This process does not show it; the next open may.
    /// - Any other `Err`: the commit record was never queued, and the
    ///   transaction is certainly rolled back.
    pub fn commit(&self, mut tx: Txn) -> Result<CommitOutcome> {
        tx.ensure_open()?;
        let pending_schema = tx.pending_schema_snapshot();
        if self.volatile {
            let reserved = self.txs.reserve_commit();
            let outcome = CommitOutcome::Committed(reserved.csn());
            return Ok(self.finish_commit(&mut tx, reserved, pending_schema, None, outcome));
        }
        // The commit record can be durable before publish_commit. Hold the
        // checkpoint horizon across that gap so recovery still replays it.
        let commit_fence = self.wal.begin_page_install()?;
        if let Some(snapshot) = pending_schema.as_deref() {
            let snapshot_bytes = crate::catalog::encode_snapshot(snapshot)?;
            self.wal.append(
                WalRecordKind::Logical,
                tx.id(),
                WalPayload::CatalogSnapshot {
                    tx_id: tx.id(),
                    schema_epoch: snapshot.meta.schema_epoch.0,
                    snapshot: snapshot_bytes,
                }
                .encode()?,
            )?;
        }

        // A failure before the commit record is queued is a certain abort.
        // `append_commit` gives the reserved CSN up on such a failure.
        let (reserved, append) = match self
            .wal
            .append_commit(tx.id(), || self.txs.reserve_commit())
        {
            Ok(value) => value,
            Err(err) => {
                self.abort_after_failed_commit(&mut tx);
                return Err(err);
            }
        };

        // Durability barrier runs before publish so a failed fsync cannot
        // leave a CSN visible to other snapshots. The previous Strict fast
        // path published, released locks, then panicked on flush failure.
        let live_durability = self.commit_durability();
        let commit_barrier = match live_durability {
            CommitDurability::Strict => self.wal.flush_until(append.end_lsn),
            CommitDurability::Normal => self.wal.write_until(append.end_lsn),
            CommitDurability::UnsafeDev => Ok(append.end_lsn),
        };
        if let Err(err) = commit_barrier {
            // The record is queued, so the WAL may hold it whole even though
            // the barrier failed, and then the next open recovers it as
            // committed. Only a record known durable is certainly committed.
            let durable = self
                .wal
                .durable_lsn()
                .is_ok_and(|durable| durable >= append.end_lsn);
            if !durable {
                // Not visible here: the writer stopped, so no later commit
                // can be made durable over it either. Give the CSN up so
                // the published frontier does not wait on it.
                let tx_id = tx.id();
                drop(reserved);
                self.abort_after_failed_commit(&mut tx);
                return Err(Error::CommitOutcomeUnknown {
                    tx_id,
                    end_lsn: append.end_lsn,
                    cause: Box::new(err),
                });
            }
        }

        #[cfg(test)]
        crate::wal::run_before_commit_publish_hook();

        let outcome = injected_commit_outcome(reserved.csn());
        Ok(self.finish_commit(
            &mut tx,
            reserved,
            pending_schema,
            Some(commit_fence),
            outcome,
        ))
    }

    /// Publish a commit whose record passed the barrier, then wait until new
    /// snapshots see it (workplan R8).
    ///
    /// CSNs publish in order: a snapshot sees commits up to the highest CSN
    /// below which every commit has published. Returning as soon as this
    /// commit published would let a caller's next transaction miss its own
    /// commit while an earlier CSN is still between its barrier and its
    /// publish. The wait comes after the locks are released: the commit it
    /// waits for needs none of them, and a failed commit gives its CSN up.
    ///
    /// The schema and index handles are published with the CSN, not gated
    /// by it: a statement that starts while this commit waits can see a new
    /// table before the rows this transaction put in it.
    fn finish_commit(
        &self,
        tx: &mut Txn,
        reserved: ReservedCsn<'_>,
        pending_schema: Option<Arc<crate::catalog::SchemaSnapshot>>,
        commit_fence: Option<PageInstallFence>,
        outcome: CommitOutcome,
    ) -> CommitOutcome {
        let csn = reserved.csn();
        reserved.publish(tx.id());
        if let Some(snapshot) = pending_schema {
            if !self.volatile {
                let _ = self.catalog_store.save_atomic(&snapshot);
            }
            self.catalog.publish(snapshot);
        }
        if !tx.pending_index_handles().is_empty()
            && let Ok(mut handles) = self.index_handles.lock()
        {
            for action in tx.pending_index_handles() {
                match action {
                    PendingIndexHandle::Install(index_id, handle) => {
                        handles.insert(*index_id, Arc::clone(handle));
                    }
                    PendingIndexHandle::Remove(index_id) => {
                        handles.remove(index_id);
                    }
                }
            }
        }
        // A checkpoint from here on records this commit in its transaction
        // status and its catalog, so it may pass the commit record. Release
        // the horizon before waiting on other commits.
        drop(commit_fence);
        self.release_locks(tx);
        tx.close();
        self.wait_until_visible(csn);
        outcome
    }

    fn wait_until_visible(&self, csn: Csn) {
        // The usual case: no earlier commit is still publishing.
        if self.txs.published_csn() >= csn {
            return;
        }
        let warn_at = Instant::now() + PUBLISH_WAIT_WARNING;
        if self.txs.wait_published(csn, Some(warn_at)) {
            return;
        }
        eprintln!(
            "redlinedb: commit csn {} has waited {}s for earlier commits to publish \
             (published csn {}); still waiting",
            csn.0,
            PUBLISH_WAIT_WARNING.as_secs(),
            self.txs.published_csn().0,
        );
        self.txs.wait_published(csn, None);
    }

    fn abort_after_failed_commit(&self, tx: &mut Txn) {
        self.txs.abort(tx.id());
        self.release_locks(tx);
        tx.close();
    }

    pub fn rollback(&self, mut tx: Txn) -> Result<()> {
        tx.ensure_open()?;
        self.txs.abort(tx.id());
        self.release_locks(&mut tx);
        tx.close();
        Ok(())
    }

    pub(super) fn refresh_read_committed(&self, tx: &mut Txn) {
        if tx.isolation() == Isolation::ReadCommitted {
            tx.replace_snapshot(self.txs.snapshot());
        }
    }

    pub(super) fn lock_row(&self, tx: &mut Txn, row_id: RowId) -> Result<()> {
        let key = crate::engine::lock::RowKey {
            rel_id: self.rel_id,
            row_id,
        };
        if tx.has_row_lock(key) {
            return Ok(());
        }
        self.locks.lock(self.rel_id, row_id, tx.id())?;
        tx.push_row_lock(key);
        Ok(())
    }

    pub(super) fn lock_row_in_rel(&self, tx: &mut Txn, rel_id: RelId, row_id: RowId) -> Result<()> {
        let key = crate::engine::lock::RowKey { rel_id, row_id };
        if tx.has_row_lock(key) {
            return Ok(());
        }
        self.locks.lock(rel_id, row_id, tx.id())?;
        tx.push_row_lock(key);
        Ok(())
    }

    pub(super) fn release_locks(&self, tx: &mut Txn) {
        let tx_id = tx.id();
        for key in tx.drain_row_locks() {
            self.locks.unlock(key.rel_id, key.row_id, tx_id);
        }
    }
}
