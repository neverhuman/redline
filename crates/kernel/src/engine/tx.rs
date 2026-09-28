use crate::catalog::IndexId as CatalogIndexId;
use crate::catalog::SchemaSnapshot;
use crate::engine::lock::RowKey;
use crate::engine::lock::RowLockManager;
use crate::format::{Csn, TxId};
use crate::index::BtreeIndex;
use crate::txn::{Isolation, Snapshot};
use crate::{Error, Result};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

#[path = "tx/status.rs"]
mod status;
pub(crate) use status::ReservedCsn;
use status::TxStatusInner;
pub use status::{ConcurrentTxStatus, TxStatusStats};

#[derive(Debug)]
pub struct Txn {
    id: TxId,
    isolation: Isolation,
    snapshot: Snapshot,
    pending_schema_snapshot: Option<Arc<SchemaSnapshot>>,
    pending_index_handles: Vec<PendingIndexHandle>,
    /// Insertion order. Release walks this so unlock order stays stable.
    row_locks: Vec<RowKey>,
    /// Membership for `has_row_lock`. The vec alone is O(n) per probe.
    row_lock_set: HashSet<RowKey>,
    /// Present for engine transactions so `Drop` can release locks that
    /// commit and rollback did not already drain.
    row_lock_manager: Option<Arc<RowLockManager>>,
    open: bool,
    lifecycle: Option<Arc<TxnLifecycle>>,
    /// A depth counter a caller may keep for nested trigger fires. The SQL
    /// crate does not use it: it tracks running trigger programs, their
    /// nesting cap and SQLite's recursion rule per connection in
    /// crates/sql/src/exec/trigger.rs, which also covers INSTEAD OF
    /// triggers that fire outside a transaction.
    trigger_depth: u32,
}

#[derive(Debug, Clone)]
pub(crate) enum PendingIndexHandle {
    Install(CatalogIndexId, Arc<BtreeIndex>),
    Remove(CatalogIndexId),
}

impl Txn {
    pub(crate) fn new(
        id: TxId,
        isolation: Isolation,
        snapshot: Snapshot,
        lifecycle: Arc<TxnLifecycle>,
    ) -> Self {
        Self {
            id,
            isolation,
            snapshot,
            pending_schema_snapshot: None,
            pending_index_handles: Vec::new(),
            row_locks: Vec::new(),
            row_lock_set: HashSet::new(),
            row_lock_manager: None,
            open: true,
            lifecycle: Some(lifecycle),
            trigger_depth: 0,
        }
    }

    /// Current value of the trigger depth counter (see the field).
    pub fn trigger_depth(&self) -> u32 {
        self.trigger_depth
    }

    /// Increment the trigger recursion depth and return the new value.
    /// Saturates at u32::MAX rather than wrapping, which would let a
    /// runaway nested-fire chain wrap to zero and bypass the cap.
    pub fn increment_trigger_depth(&mut self) -> u32 {
        self.trigger_depth = self.trigger_depth.saturating_add(1);
        self.trigger_depth
    }

    /// Decrement the trigger recursion depth (clamped at 0). Called when
    /// a trigger body completes.
    pub fn decrement_trigger_depth(&mut self) {
        self.trigger_depth = self.trigger_depth.saturating_sub(1);
    }

    pub fn id(&self) -> TxId {
        self.id
    }

    pub fn isolation(&self) -> Isolation {
        self.isolation
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub(crate) fn replace_snapshot(&mut self, snapshot: Snapshot) {
        if let Some(lifecycle) = &self.lifecycle {
            lifecycle.update_snapshot(snapshot.visible_csn);
        }
        self.snapshot = snapshot;
    }

    pub(crate) fn set_pending_schema_snapshot(&mut self, snapshot: Arc<SchemaSnapshot>) {
        self.pending_schema_snapshot = Some(snapshot);
    }

    pub(crate) fn pending_schema_snapshot(&self) -> Option<Arc<SchemaSnapshot>> {
        self.pending_schema_snapshot.as_ref().map(Arc::clone)
    }

    pub(crate) fn push_pending_index_handle(&mut self, action: PendingIndexHandle) {
        self.pending_index_handles.push(action);
    }

    pub(crate) fn pending_index_handles(&self) -> &[PendingIndexHandle] {
        &self.pending_index_handles
    }

    pub(crate) fn ensure_open(&self) -> Result<()> {
        if self.open {
            Ok(())
        } else {
            Err(Error::TransactionClosed)
        }
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
        if let Some(lifecycle) = self.lifecycle.take() {
            lifecycle.close();
        }
    }

    pub(crate) fn has_row_lock(&self, key: RowKey) -> bool {
        crate::observe::add_row_lock_probe();
        self.row_lock_set.contains(&key)
    }

    pub(crate) fn push_row_lock(&mut self, key: RowKey) {
        if self.row_lock_set.insert(key) {
            self.row_locks.push(key);
        }
    }

    /// Forget `key` without unlocking it; its lock now belongs to another
    /// transaction (`Engine::transfer_begin_lock`).
    pub(crate) fn remove_row_lock(&mut self, key: RowKey) {
        if self.row_lock_set.remove(&key) {
            self.row_locks.retain(|held| *held != key);
        }
    }

    pub(crate) fn drain_row_locks(&mut self) -> impl Iterator<Item = RowKey> + '_ {
        self.row_lock_set.clear();
        self.row_locks.drain(..)
    }

    pub(crate) fn attach_row_lock_manager(&mut self, locks: Arc<RowLockManager>) {
        self.row_lock_manager = Some(locks);
    }
}

impl Drop for Txn {
    fn drop(&mut self) {
        // Abort before unlock so a waiter does not observe this transaction
        // as still in progress. Commit and rollback already closed it.
        if let Some(lifecycle) = &self.lifecycle {
            lifecycle.abort_if_open();
        }
        let Some(locks) = self.row_lock_manager.take() else {
            return;
        };
        let tx_id = self.id;
        for key in self.drain_row_locks() {
            locks.unlock(key.rel_id, key.row_id, tx_id);
        }
    }
}

#[derive(Debug)]
pub(crate) struct TxnLifecycle {
    tx_id: TxId,
    inner: Weak<TxStatusInner>,
    closed: AtomicBool,
}

impl TxnLifecycle {
    fn update_snapshot(&self, csn: Csn) {
        if let Some(inner) = self.inner.upgrade() {
            inner.set_active_snapshot(self.tx_id, csn);
        }
    }

    fn close(&self) {
        // Publish and abort already dropped the active snapshot.
        self.closed.store(true, Ordering::SeqCst);
    }

    fn abort_if_open(&self) {
        if !self.closed.swap(true, Ordering::SeqCst)
            && let Some(inner) = self.inner.upgrade()
        {
            inner.abort(self.tx_id);
        }
    }
}

impl Drop for TxnLifecycle {
    fn drop(&mut self) {
        self.abort_if_open();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::lock::RowKey;
    use crate::format::{RelId, RowId};

    #[test]
    fn row_locks_track_relation_identity() {
        let txs = ConcurrentTxStatus::new();
        let mut tx = txs.begin_txn(Isolation::Snapshot);
        let first = RowKey {
            rel_id: RelId(1),
            row_id: RowId(7),
        };
        let second = RowKey {
            rel_id: RelId(2),
            row_id: RowId(7),
        };
        tx.push_row_lock(first);
        assert!(tx.has_row_lock(first));
        assert!(!tx.has_row_lock(second));
        let locked: Vec<_> = tx.drain_row_locks().collect();
        assert_eq!(locked, vec![first]);
        assert!(!tx.has_row_lock(first));
    }

    #[test]
    fn row_lock_membership_ignores_duplicates_and_keeps_order() {
        let txs = ConcurrentTxStatus::new();
        let mut tx = txs.begin_txn(Isolation::Snapshot);
        let keys: Vec<RowKey> = (0..64)
            .map(|i| RowKey {
                rel_id: RelId(3),
                row_id: RowId(i),
            })
            .collect();
        for key in &keys {
            tx.push_row_lock(*key);
            tx.push_row_lock(*key);
        }
        assert!(tx.has_row_lock(keys[40]));
        assert!(!tx.has_row_lock(RowKey {
            rel_id: RelId(9),
            row_id: RowId(40),
        }));
        let locked: Vec<_> = tx.drain_row_locks().collect();
        assert_eq!(locked, keys);
    }

    #[test]
    fn commit_rollback_and_drop_unregister_once() {
        use std::time::Duration;

        use tempfile::TempDir;

        use crate::engine::{CommitDurability, Engine, EngineConfig};
        use crate::format::RelId;
        use crate::txn::Isolation;

        use super::status::take_active_unregisters;

        let dir = TempDir::new().unwrap();
        let engine = Engine::create(
            dir.path(),
            EngineConfig {
                rel_id: RelId(1),
                commit_durability: CommitDurability::UnsafeDev,
                busy_timeout: Duration::from_millis(50),
                ..EngineConfig::default()
            },
        )
        .unwrap();
        let _ = take_active_unregisters();
        let tx = engine.begin(Isolation::Snapshot).unwrap();
        engine.commit(tx).unwrap();
        assert_eq!(take_active_unregisters(), 1);
        let tx = engine.begin(Isolation::Snapshot).unwrap();
        engine.rollback(tx).unwrap();
        assert_eq!(take_active_unregisters(), 1);
        let tx = engine.begin(Isolation::Snapshot).unwrap();
        drop(tx);
        assert_eq!(take_active_unregisters(), 1);
    }
}
