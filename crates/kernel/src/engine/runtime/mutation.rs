use super::*;
use crate::engine::page_heap::HeapScanRow;

impl Engine {
    pub fn get(&self, tx: &mut Txn, row_id: RowId) -> Result<Option<Vec<u8>>> {
        tx.ensure_open()?;
        self.refresh_read_committed(tx);
        let snapshot = tx.snapshot().clone();
        self.heap.get(&self.txs, &snapshot, Some(tx.id()), row_id)
    }

    pub fn get_for_relation(
        &self,
        tx: &mut Txn,
        rel_id: RelId,
        row_id: RowId,
    ) -> Result<Option<Vec<u8>>> {
        tx.ensure_open()?;
        crate::observe::add_relation_get();
        self.refresh_read_committed(tx);
        let snapshot = tx.snapshot().clone();
        self.heap
            .get_for_relation(&self.txs, &snapshot, Some(tx.id()), rel_id, row_id)
    }

    /// Row `row_id` of `rel_id` as the latest committed state and `tx`'s own
    /// writes show it, whatever `tx`'s snapshot, so a row another transaction
    /// committed after `tx` began counts. Rowid allocation reads it to skip a
    /// rowid a row still holds.
    pub fn get_for_relation_latest(
        &self,
        tx: &Txn,
        rel_id: RelId,
        row_id: RowId,
    ) -> Result<Option<Vec<u8>>> {
        tx.ensure_open()?;
        crate::observe::add_relation_get();
        let latest = self.txs.snapshot();
        self.heap
            .get_for_relation(&self.txs, &latest, Some(tx.id()), rel_id, row_id)
    }

    /// Every row of `rel_id` that `tx` sees, read by `workers` threads that
    /// split the relation's heap pages between them. Each row is read as
    /// [`Engine::get_for_relation`] reads it: one version, never a
    /// superseded or deleted one. Row order is not stable. The SQL parallel
    /// covering-scan dispatch calls this inside `pool.install(|| ...)` so
    /// the workers run in the database's Rayon pool.
    pub fn parallel_scan_relation(
        &self,
        tx: &mut Txn,
        rel_id: RelId,
        workers: usize,
    ) -> Result<Vec<HeapScanRow>> {
        tx.ensure_open()?;
        self.refresh_read_committed(tx);
        let snapshot = tx.snapshot().clone();
        self.heap.parallel_scan_relation(
            &self.txs,
            &snapshot,
            Some(tx.id()),
            Some(rel_id),
            workers,
            None,
        )
    }

    pub fn insert(&self, tx: &mut Txn, payload: Vec<u8>) -> Result<RowId> {
        tx.ensure_open()?;
        self.refresh_read_committed(tx);
        let row_id = self.heap.reserve_row_id();
        // LSN sentinel: mutation. The heap append_cell logs a PageImage with
        // the real WAL end-LSN; this argument only flags the page as dirty.
        self.heap
            .insert_with_row_id(tx.id(), row_id, payload, Lsn(1))?;
        Ok(row_id)
    }

    pub fn insert_for_relation(
        &self,
        tx: &mut Txn,
        rel_id: RelId,
        row_id: RowId,
        payload: Vec<u8>,
    ) -> Result<()> {
        tx.ensure_open()?;
        self.refresh_read_committed(tx);
        self.heap
            .insert_for_relation(tx.id(), rel_id, row_id, payload, Lsn(1))
    }

    pub fn reserve_row_id(&self) -> RowId {
        self.heap.reserve_row_id()
    }

    /// Take the next rowid of `rel_id`; each relation numbers its own rows.
    pub fn reserve_row_id_for(&self, rel_id: RelId) -> Result<RowId> {
        self.heap.reserve_row_id_for(rel_id)
    }

    /// The rowid `rel_id` hands out next.
    pub fn relation_next_row(&self, rel_id: RelId) -> Result<u64> {
        self.heap.relation_next_row(rel_id)
    }

    /// Lower the next rowid of `rel_id` from `expected` to `next_row`, as a
    /// delete of the relation's highest rowid does. Returns `false`, and
    /// changes nothing, when the counter no longer holds `expected`.
    pub fn lower_relation_next_row(
        &self,
        rel_id: RelId,
        expected: u64,
        next_row: u64,
    ) -> Result<bool> {
        self.heap
            .lower_relation_next_row(rel_id, expected, next_row)
    }

    pub fn lower_next_row(&self, next_row: u64) {
        self.heap.lower_next_row(next_row)
    }

    pub fn insert_with_row_id(&self, tx: &mut Txn, row_id: RowId, payload: Vec<u8>) -> Result<()> {
        tx.ensure_open()?;
        self.refresh_read_committed(tx);
        self.heap
            .insert_with_row_id(tx.id(), row_id, payload, Lsn(1))
    }

    pub fn update(&self, tx: &mut Txn, row_id: RowId, payload: Vec<u8>) -> Result<()> {
        tx.ensure_open()?;
        self.refresh_read_committed(tx);
        self.lock_row(tx, row_id)?;
        self.refresh_read_committed(tx);
        self.heap
            .update(tx.id(), tx.snapshot(), &self.txs, row_id, payload, Lsn(1))
    }

    pub fn update_for_relation(
        &self,
        tx: &mut Txn,
        rel_id: RelId,
        row_id: RowId,
        payload: Vec<u8>,
    ) -> Result<()> {
        tx.ensure_open()?;
        self.refresh_read_committed(tx);
        self.lock_row_in_rel(tx, rel_id, row_id)?;
        self.refresh_read_committed(tx);
        self.heap.update_for_relation(
            tx.id(),
            tx.snapshot(),
            &self.txs,
            RelationWriteTarget { rel_id, row_id },
            payload,
            Lsn(1),
        )
    }

    pub fn lock_row_for_relation(&self, tx: &mut Txn, rel_id: RelId, row_id: RowId) -> Result<()> {
        tx.ensure_open()?;
        self.refresh_read_committed(tx);
        self.lock_row_in_rel(tx, rel_id, row_id)?;
        self.refresh_read_committed(tx);
        Ok(())
    }

    pub fn delete(&self, tx: &mut Txn, row_id: RowId) -> Result<()> {
        tx.ensure_open()?;
        self.refresh_read_committed(tx);
        self.lock_row(tx, row_id)?;
        self.refresh_read_committed(tx);
        self.heap
            .delete(tx.id(), tx.snapshot(), &self.txs, row_id, Lsn(1))
    }

    pub fn delete_for_relation(&self, tx: &mut Txn, rel_id: RelId, row_id: RowId) -> Result<()> {
        tx.ensure_open()?;
        self.refresh_read_committed(tx);
        self.lock_row_in_rel(tx, rel_id, row_id)?;
        self.refresh_read_committed(tx);
        self.heap
            .delete_for_relation(tx.id(), tx.snapshot(), &self.txs, rel_id, row_id, Lsn(1))
    }
}
