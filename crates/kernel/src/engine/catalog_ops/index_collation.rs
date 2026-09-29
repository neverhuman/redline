//! Bringing an existing index's key collations in line with the NOCASE or
//! RTRIM its columns declare (workplan Q5-10, step 4).
//!
//! A database written before key collations were inherited has indexes
//! whose keys compare BINARY on such columns. An index with a B-tree is
//! rebuilt at open like one at an older index-format epoch
//! ([`Engine::indexes_needing_rebuild`] lists it, and every rebuild applies
//! the inherited collations). An index without a B-tree (the UNIQUE and
//! PRIMARY KEY constraints of a table, checked by scanning the table) only
//! needs its catalog entry changed, after the caller has checked that no
//! two rows now share a key.

use super::*;

impl Engine {
    /// Catalog indexes whose keys should inherit a NOCASE or RTRIM their
    /// columns declare (see
    /// [`crate::catalog::collation::index_keys_needing_inherited_collation`]).
    pub fn indexes_needing_inherited_collation(&self) -> Vec<CatalogIndexId> {
        let snapshot = self.catalog.current();
        snapshot
            .indexes
            .iter()
            .filter(|index| {
                snapshot.table_by_id(index.table_id).is_some_and(|table| {
                    crate::catalog::collation::index_keys_needing_inherited_collation(&table, index)
                        .is_some()
                })
            })
            .map(|index| index.index_id)
            .collect()
    }

    /// Give index `index_id` its inherited key collations inside `tx`,
    /// without touching any B-tree. For an index with a B-tree use a
    /// rebuild instead, which applies them too. The caller must check the
    /// table's rows under the new collations before it commits.
    pub fn inherit_index_key_collations(
        &self,
        tx: &mut Txn,
        index_id: CatalogIndexId,
    ) -> Result<Arc<crate::catalog::IndexDef>> {
        tx.ensure_open()?;
        let _ddl = self.catalog.lock_ddl();
        let snapshot = self.catalog_snapshot_for_tx(tx);
        let next = Arc::new(crate::catalog::apply_inherited_index_key_collations(
            (*snapshot).clone(),
            index_id,
        )?);
        let updated = next
            .index_by_id(index_id)
            .ok_or_else(|| Error::ObjectNotFound)?;
        tx.set_pending_schema_snapshot(next);
        Ok(updated)
    }
}
