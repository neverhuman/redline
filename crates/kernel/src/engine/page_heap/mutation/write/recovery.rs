use super::PageBackedHeap;
use crate::engine::page_heap::advance_atomic_past;
use crate::format::{Lsn, RelId, RowId, TUPLE_FLAG_DELETED, TxId};
use crate::txn::{UndoKind, UndoRecord};
use crate::wal::WalPayload;
use crate::{Result, format::TupleVersion};

impl PageBackedHeap {
    pub fn insert_with_row_id(
        &self,
        tx_id: TxId,
        row_id: RowId,
        payload: Vec<u8>,
        lsn: Lsn,
    ) -> Result<()> {
        advance_atomic_past(&self.next_row, row_id.0);
        self.insert_recovered_at(tx_id, self.rel_id, row_id, payload, lsn)
    }

    pub fn insert_for_relation(
        &self,
        tx_id: TxId,
        rel_id: RelId,
        row_id: RowId,
        payload: Vec<u8>,
        lsn: Lsn,
    ) -> Result<()> {
        advance_atomic_past(&self.next_row, row_id.0);
        self.insert_recovered_at(tx_id, rel_id, row_id, payload, lsn)
    }

    pub fn insert_recovered(&self, tx_id: TxId, row_id: RowId, payload: Vec<u8>) -> Result<()> {
        self.insert_recovered_at(tx_id, self.rel_id, row_id, payload, Lsn::ZERO)
    }

    pub fn insert_recovered_for_relation(
        &self,
        tx_id: TxId,
        rel_id: RelId,
        row_id: RowId,
        payload: Vec<u8>,
    ) -> Result<()> {
        self.insert_recovered_at(tx_id, rel_id, row_id, payload, Lsn::ZERO)
    }

    pub(crate) fn insert_recovered_at(
        &self,
        tx_id: TxId,
        rel_id: RelId,
        row_id: RowId,
        payload: Vec<u8>,
        lsn: Lsn,
    ) -> Result<()> {
        advance_atomic_past(&self.next_row, row_id.0);
        let rel_id = if rel_id == RelId::ZERO {
            self.rel_id
        } else {
            rel_id
        };
        let current = match self.head_for_relation(rel_id, row_id)? {
            Some(ptr) => self.read_tuple(ptr).ok(),
            None => None,
        };
        if let Some(existing) = &current
            && existing.begin_tx == tx_id
            && existing.row_id == row_id
            && existing.payload == payload
            && existing.flags & TUPLE_FLAG_DELETED == 0
        {
            return Ok(());
        }
        let wal_payload = if lsn != Lsn::ZERO {
            Some(WalPayload::HeapInsert {
                tx_id,
                rel_id,
                row_id,
                payload: payload.clone(),
            })
        } else {
            None
        };
        let mut tuple = TupleVersion::new(row_id, rel_id, tx_id, payload);
        if let Some(current) = current {
            // The row id is taken again, usually after a delete. The new
            // version replaces the current one, a tombstone then, as an update
            // would, so link it to that version's before-image. Older
            // snapshots then still reach the versions under it, and when the
            // row directory is rebuilt from pages the link says which of one
            // transaction's versions came last; their page positions cannot,
            // once a version lands on a reused page with a lower id.
            let mut before = current.clone();
            before.end_tx = tx_id;
            tuple.undo_head = self.append_undo(
                tx_id,
                row_id,
                UndoRecord {
                    kind: UndoKind::UpdateBeforeImage,
                    tx_id,
                    row_id,
                    prev_undo: current.undo_head,
                    before_image: before.encode()?,
                },
                lsn,
            )?;
        }
        let ptr = self.append_tuple(tx_id, row_id, tuple, lsn, wal_payload)?;
        self.set_head(row_id, ptr)?;
        self.set_relation_head(rel_id, row_id, ptr)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::PageBackedHeap;
    use crate::format::{Lsn, PageId, RelId, RowId, TxId};
    use crate::storage::{BufferPool, PageFile};
    use crate::wal::{WalConfig, WalCoordinator};

    #[test]
    fn replaying_a_heap_insert_does_not_append_a_second_cell() {
        let dir = tempfile::tempdir().unwrap();
        let page_file = Arc::new(PageFile::create(dir.path().join("data.redline"), 4096).unwrap());
        let buffer = Arc::new(BufferPool::new(page_file, 32).unwrap());
        let wal =
            Arc::new(WalCoordinator::create(dir.path().join("wal"), WalConfig::default()).unwrap());
        let heap = PageBackedHeap::new_with_wal(RelId(1), 1, buffer, Some(wal)).unwrap();
        let payload = b"row".to_vec();
        heap.insert_for_relation(TxId(1), RelId(1), RowId(7), payload.clone(), Lsn(1))
            .unwrap();
        let slots = heap
            .buffer
            .pin(PageId(1))
            .unwrap()
            .with_page(|page| page.slot_count())
            .unwrap();
        heap.insert_recovered_for_relation(TxId(1), RelId(1), RowId(7), payload)
            .unwrap();
        let again = heap
            .buffer
            .pin(PageId(1))
            .unwrap()
            .with_page(|page| page.slot_count())
            .unwrap();
        assert_eq!(again, slots);
        assert_eq!(slots, 1);
        let head = heap.head_for_relation(RelId(1), RowId(7)).unwrap();
        let tuple = heap.read_tuple(head.unwrap()).unwrap();
        assert_eq!(tuple.payload, b"row");
    }
}
