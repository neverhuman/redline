//! Index rebuild: the one mechanism behind the open-time index-format
//! upgrade and `REINDEX`.
//!
//! A rebuild never edits the old B-tree. It allocates a new one at the
//! current index-format epoch, fills it from the heap, and swaps the
//! catalog's `meta_page_id` inside the caller's transaction, so the index is
//! either wholly old or wholly rebuilt: a crash before COMMIT leaves the old
//! B-tree (and, for an upgrade, its old epoch, so the next open rebuilds
//! again), and a crash after COMMIT recovers the new one. The new B-tree gets
//! a fresh physical id, so WAL records the old B-tree wrote (in the old key
//! format, or describing entries the rebuild already reflects) are never
//! replayed into it. The old B-tree's pages are not reclaimed, like those of
//! a dropped index.

use super::*;

impl Engine {
    /// Catalog indexes whose B-tree was written at an older index-format
    /// epoch. They have no open handle; the SQL layer rebuilds them before it
    /// hands the database out.
    pub fn indexes_needing_rebuild(&self) -> Result<Vec<CatalogIndexId>> {
        let snapshot = self.catalog.current();
        let current = current_index_version();
        let mut outdated = Vec::new();
        for index in &snapshot.indexes {
            let Some(meta_page_id) = index.meta_page_id else {
                continue;
            };
            let version = BtreeIndex::format_version(&self.buffer, meta_page_id)?;
            if version < current {
                outdated.push(index.index_id);
            }
        }
        Ok(outdated)
    }

    /// Give index `index_id` a new, empty B-tree at the current index-format
    /// epoch inside `tx`. The catalog's `meta_page_id` and the engine's
    /// handle switch to it only if `tx` commits; until then `tx` sees the new
    /// handle through [`Engine::index_handle_for_tx`] and every other
    /// transaction keeps the old one. The caller fills the B-tree:
    /// [`Engine::rebuild_index`] does it for column indexes, and the SQL
    /// layer does it for every index (it alone evaluates expression keys and
    /// partial-index predicates).
    pub fn rebuild_index_empty(
        &self,
        tx: &mut Txn,
        index_id: CatalogIndexId,
    ) -> Result<Arc<crate::catalog::IndexDef>> {
        self.rebuild_index_inner(tx, index_id, false)
    }

    /// [`Engine::rebuild_index_empty`] plus the heap backfill for an index
    /// keyed on plain columns. An expression or partial index is refused,
    /// because the kernel cannot evaluate SQL; rebuild those through the SQL
    /// layer. A UNIQUE index whose rows now share a key fails with
    /// [`Error::ConstraintViolation`].
    pub fn rebuild_index(
        &self,
        tx: &mut Txn,
        index_id: CatalogIndexId,
    ) -> Result<Arc<crate::catalog::IndexDef>> {
        self.rebuild_index_inner(tx, index_id, true)
    }

    /// Kernel-only open-time upgrade: rebuild every index at an older epoch
    /// in one transaction with [`Engine::rebuild_index`]. The SQL layer does
    /// its own upgrade (which also handles expression and partial indexes);
    /// this is for embedders that drive the kernel directly. Returns how
    /// many indexes were rebuilt.
    pub fn rebuild_stale_indexes(&self) -> Result<usize> {
        let outdated = self.indexes_needing_rebuild()?;
        if outdated.is_empty() {
            return Ok(0);
        }
        let mut tx = self.begin(Isolation::Snapshot)?;
        for &index_id in &outdated {
            if let Err(err) = self.rebuild_index(&mut tx, index_id) {
                self.rollback(tx)?;
                return Err(err);
            }
        }
        match self.commit(tx)? {
            CommitOutcome::Committed(_) => Ok(outdated.len()),
            CommitOutcome::MaybeCommitted => {
                Err(Error::CorruptWal("index rebuild maybe committed"))
            }
            CommitOutcome::RolledBack => Err(Error::CorruptWal("index rebuild rolled back")),
        }
    }

    fn rebuild_index_inner(
        &self,
        tx: &mut Txn,
        index_id: CatalogIndexId,
        kernel_backfill: bool,
    ) -> Result<Arc<crate::catalog::IndexDef>> {
        tx.ensure_open()?;
        let _ddl = self.catalog.lock_ddl();
        let snapshot = self.catalog_snapshot_for_tx(tx);
        let index = snapshot
            .index_by_id(index_id)
            .ok_or(Error::ObjectNotFound)?;
        if kernel_backfill
            && (index.predicate_sql.is_some()
                || index.keys.iter().any(|key| {
                    matches!(
                        key.source,
                        crate::catalog::IndexKeySource::Expression { .. }
                    )
                }))
        {
            return Err(Error::UnsupportedDdl(
                "the kernel cannot rebuild an expression or partial index; rebuild it through the SQL layer",
            ));
        }
        let mut next = (*snapshot).clone();
        // A fresh object id, never reused, keeps the old B-tree's WAL
        // records out of the new one during recovery.
        let physical_id = PhysicalIndexId(next.meta.next_object_id.0);
        next.meta.next_object_id.0 += 1;
        let descriptor = IndexDescriptor::new(
            physical_id,
            index.relation_id,
            if index.unique {
                IndexUniqueness::Unique
            } else {
                IndexUniqueness::NonUnique
            },
        );
        let btree =
            BtreeIndex::create_with_wal(Arc::clone(&self.buffer), descriptor, self.page_wal())?;
        btree.set_phase11_counters(Arc::clone(&self.phase11_counters));
        btree.record_initial_page_images(tx.id())?;
        let next = Arc::new(apply_set_index_meta_page_id(
            next,
            index_id,
            btree.meta_page_id(),
        )?);
        let rebuilt = next
            .index_by_id(index_id)
            .ok_or(Error::CatalogCorrupt("rebuilt index missing from snapshot"))?;
        if kernel_backfill {
            let table = next
                .table_by_id(rebuilt.table_id)
                .ok_or(Error::ObjectNotFound)?;
            self.backfill_index(tx, &btree, &table, &rebuilt)?;
        }
        tx.push_pending_index_handle(PendingIndexHandle::Install(index_id, Arc::new(btree)));
        tx.set_pending_schema_snapshot(next);
        Ok(rebuilt)
    }
}
