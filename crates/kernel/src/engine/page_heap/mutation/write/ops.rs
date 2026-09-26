use super::{PageBackedHeap, RelationWriteTarget};
use crate::Result;
use crate::engine::tx::ConcurrentTxStatus;
use crate::format::{Lsn, RelId, RowId, TUPLE_FLAG_DELETED, TxId};
use crate::txn::Snapshot;

impl PageBackedHeap {
    pub fn update(
        &self,
        tx_id: TxId,
        snapshot: &Snapshot,
        tx_status: &ConcurrentTxStatus,
        row_id: RowId,
        payload: Vec<u8>,
        lsn: Lsn,
    ) -> Result<()> {
        let current = self.visible_tuple_for_write(tx_id, snapshot, tx_status, row_id)?;
        self.append_update_version(tx_id, self.rel_id, row_id, payload, current, lsn)
    }

    pub fn update_for_relation(
        &self,
        tx_id: TxId,
        snapshot: &Snapshot,
        tx_status: &ConcurrentTxStatus,
        target: RelationWriteTarget,
        payload: Vec<u8>,
        lsn: Lsn,
    ) -> Result<()> {
        let current =
            self.visible_tuple_for_write_in_relation(tx_id, snapshot, tx_status, target)?;
        self.append_update_version(tx_id, target.rel_id, target.row_id, payload, current, lsn)
    }

    pub fn update_recovered(&self, tx_id: TxId, row_id: RowId, payload: Vec<u8>) -> Result<()> {
        self.update_recovered_for_relation(tx_id, self.rel_id, row_id, payload)
    }

    pub fn update_recovered_for_relation(
        &self,
        tx_id: TxId,
        rel_id: RelId,
        row_id: RowId,
        payload: Vec<u8>,
    ) -> Result<()> {
        let current = self.current_tuple_recovered(rel_id, row_id)?;
        if current.begin_tx == tx_id
            && current.payload == payload
            && current.flags & TUPLE_FLAG_DELETED == 0
        {
            return Ok(());
        }
        self.append_update_version(tx_id, rel_id, row_id, payload, current, Lsn::ZERO)
    }

    pub fn delete(
        &self,
        tx_id: TxId,
        snapshot: &Snapshot,
        tx_status: &ConcurrentTxStatus,
        row_id: RowId,
        lsn: Lsn,
    ) -> Result<()> {
        let current = self.visible_tuple_for_write(tx_id, snapshot, tx_status, row_id)?;
        self.append_delete_version(tx_id, self.rel_id, row_id, current, lsn)
    }

    pub fn delete_for_relation(
        &self,
        tx_id: TxId,
        snapshot: &Snapshot,
        tx_status: &ConcurrentTxStatus,
        rel_id: RelId,
        row_id: RowId,
        lsn: Lsn,
    ) -> Result<()> {
        let current = self.visible_tuple_for_write_in_relation(
            tx_id,
            snapshot,
            tx_status,
            RelationWriteTarget { rel_id, row_id },
        )?;
        self.append_delete_version(tx_id, rel_id, row_id, current, lsn)
    }

    pub fn delete_recovered(&self, tx_id: TxId, row_id: RowId) -> Result<()> {
        self.delete_recovered_for_relation(tx_id, self.rel_id, row_id)
    }

    pub fn delete_recovered_for_relation(
        &self,
        tx_id: TxId,
        rel_id: RelId,
        row_id: RowId,
    ) -> Result<()> {
        let current = self.current_tuple_recovered(rel_id, row_id)?;
        if current.begin_tx == tx_id && current.flags & TUPLE_FLAG_DELETED != 0 {
            return Ok(());
        }
        self.append_delete_version(tx_id, rel_id, row_id, current, Lsn::ZERO)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::PageBackedHeap;
    use crate::format::{Lsn, PageId, RelId, RowId, TUPLE_FLAG_DELETED, TxId};
    use crate::storage::{BufferPool, PageFile};
    use crate::wal::{WalConfig, WalCoordinator};

    fn heap() -> (tempfile::TempDir, PageBackedHeap) {
        let dir = tempfile::tempdir().unwrap();
        let page_file = Arc::new(PageFile::create(dir.path().join("data.redline"), 4096).unwrap());
        let buffer = Arc::new(BufferPool::new(page_file, 32).unwrap());
        let wal =
            Arc::new(WalCoordinator::create(dir.path().join("wal"), WalConfig::default()).unwrap());
        let heap = PageBackedHeap::new_with_wal(RelId(1), 1, buffer, Some(wal)).unwrap();
        (dir, heap)
    }

    fn slots(heap: &PageBackedHeap) -> u16 {
        heap.buffer
            .pin(PageId(1))
            .unwrap()
            .with_page(|page| page.slot_count())
            .unwrap()
    }

    #[test]
    fn replaying_an_update_or_delete_does_not_append_another_version() {
        let (_dir, heap) = heap();
        heap.insert_for_relation(TxId(1), RelId(1), RowId(4), b"a".to_vec(), Lsn(1))
            .unwrap();
        heap.update_recovered_for_relation(TxId(2), RelId(1), RowId(4), b"b".to_vec())
            .unwrap();
        let after_update = slots(&heap);
        heap.update_recovered_for_relation(TxId(2), RelId(1), RowId(4), b"b".to_vec())
            .unwrap();
        assert_eq!(slots(&heap), after_update);
        heap.update_recovered_for_relation(TxId(3), RelId(1), RowId(4), b"c".to_vec())
            .unwrap();
        assert!(slots(&heap) > after_update);

        heap.delete_recovered_for_relation(TxId(4), RelId(1), RowId(4))
            .unwrap();
        let after_delete = slots(&heap);
        heap.delete_recovered_for_relation(TxId(4), RelId(1), RowId(4))
            .unwrap();
        assert_eq!(slots(&heap), after_delete);
        let head = heap.head_for_relation(RelId(1), RowId(4)).unwrap().unwrap();
        let tuple = heap.read_tuple(head).unwrap();
        assert_ne!(tuple.flags & TUPLE_FLAG_DELETED, 0);
    }
}
