use super::*;
use crate::format::{RelId, RowId};

/// The row-lock key a schema change holds until its transaction ends. No
/// relation uses this id.
const SCHEMA_LOCK_REL: RelId = RelId(u64::MAX);

impl Engine {
    /// Lock the schema for `tx`'s change to it: the schema lock, held until
    /// `tx` commits or rolls back, then the catalog's own lock for the
    /// statement. A schema change builds the next catalog from `tx`'s view
    /// and publishes it at commit; two transactions changing the schema at
    /// once would both build on the same catalog, give their objects the
    /// same ids, and the second commit would replace the first one's
    /// catalog. With the schema lock the second waits (up to the busy
    /// timeout) until the first ends, and then builds on what it published.
    pub(super) fn lock_schema(&self, tx: &mut Txn) -> Result<parking_lot::MutexGuard<'_, ()>> {
        self.lock_row_in_rel(tx, SCHEMA_LOCK_REL, RowId(0))?;
        Ok(self.catalog.lock_ddl())
    }

    pub(super) fn catalog_snapshot_for_tx(&self, tx: &Txn) -> Arc<crate::catalog::SchemaSnapshot> {
        match tx.pending_schema_snapshot() {
            Some(snap) => snap,
            None => self.catalog.current(),
        }
    }
}
