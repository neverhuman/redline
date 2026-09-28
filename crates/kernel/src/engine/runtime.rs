//! Transaction runtime, row mutation, locking, and commit/rollback behavior.

use std::sync::Arc;
use std::time::Duration;

use crate::engine::page_heap::RelationWriteTarget;
use crate::engine::tx::PendingIndexHandle;
use crate::format::{Csn, Lsn, RelId, RowId};
use crate::txn::{Isolation, Snapshot};
use crate::wal::{WalPayload, WalRecordKind};
use crate::{Error, Result};

use super::{BEGIN_LOCK_KEY, CommitDurability, CommitOutcome, Engine, EngineConfig, Txn};

#[path = "runtime/commit.rs"]
mod commit;
#[path = "runtime/commit_alone.rs"]
mod commit_alone;
#[path = "runtime/mutation.rs"]
mod mutation;

#[cfg(feature = "failpoints")]
pub use commit::arm_commit_failure_for_thread;

impl Engine {
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    pub fn set_busy_timeout(&self, timeout: Duration) {
        self.locks.set_timeout(timeout);
    }

    pub fn begin(&self, isolation: Isolation) -> Result<Txn> {
        if isolation == Isolation::Serializable {
            return Err(Error::UnsupportedIsolation);
        }
        let mut tx = {
            let _gate = self.locks.begin_shared();
            self.txs.begin_txn(isolation)
        };
        tx.attach_row_lock_manager(Arc::clone(&self.locks));
        Ok(tx)
    }

    /// Begin a transaction that reads through `snapshot` instead of a fresh
    /// one: it sees what `snapshot` sees plus its own writes. The SQL
    /// layer's ROLLBACK TO has no partial undo; it begins one of these with
    /// the rolled-back transaction's snapshot, before rolling that one
    /// back, and re-runs the statements before the savepoint in it (S9-05).
    pub fn begin_with_snapshot(&self, isolation: Isolation, snapshot: Snapshot) -> Result<Txn> {
        let mut tx = self.begin(isolation)?;
        tx.replace_snapshot(snapshot);
        Ok(tx)
    }

    /// Move `from`'s BEGIN IMMEDIATE / EXCLUSIVE reservation to `to` without
    /// releasing it in between, so no other writer can reserve in the gap.
    /// Returns whether `from` held one.
    pub fn transfer_begin_lock(&self, from: &mut Txn, to: &mut Txn) -> bool {
        if !from.has_row_lock(BEGIN_LOCK_KEY)
            || !self.locks.transfer(
                BEGIN_LOCK_KEY.rel_id,
                BEGIN_LOCK_KEY.row_id,
                from.id(),
                to.id(),
            )
        {
            return false;
        }
        from.remove_row_lock(BEGIN_LOCK_KEY);
        to.push_row_lock(BEGIN_LOCK_KEY);
        true
    }

    pub fn reserve_begin_lock(&self, tx: &mut Txn) -> Result<()> {
        if tx.has_row_lock(BEGIN_LOCK_KEY) {
            return Ok(());
        }
        self.locks
            .lock(BEGIN_LOCK_KEY.rel_id, BEGIN_LOCK_KEY.row_id, tx.id())?;
        tx.push_row_lock(BEGIN_LOCK_KEY);
        Ok(())
    }
}
