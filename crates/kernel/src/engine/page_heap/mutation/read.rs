use super::{PageBackedHeap, RelationWriteTarget};
use crate::engine::page_heap::policy::{ActiveUndoReadPolicy, UndoReadContext, UndoReadPolicy};
use crate::engine::page_heap::{ConcurrentVisibility, decode_undo_ptr};
use crate::engine::tx::ConcurrentTxStatus;
use crate::format::{PageId, PageKind, RelId, RowId, TuplePtr, TupleVersion, TxId, UndoPtr};
use crate::txn::{Snapshot, TupleVisibility, TxState, UndoRecord};
use crate::{Error, Result};

/// A tuple pointer names a page generation that reuse has since replaced.
pub(crate) const STALE_TUPLE_POINTER: &str = "tuple pointer generation mismatch";

impl PageBackedHeap {
    pub fn get(
        &self,
        tx_status: &ConcurrentTxStatus,
        snapshot: &Snapshot,
        owner: Option<TxId>,
        row_id: RowId,
    ) -> Result<Option<Vec<u8>>> {
        let Some(ptr) = self.head(row_id)? else {
            return Ok(None);
        };
        let current = self.read_tuple(ptr)?;
        match current.visibility_concurrent(tx_status, snapshot, owner) {
            TupleVisibility::Visible => Ok(Some(current.payload)),
            TupleVisibility::Deleted => Ok(None),
            TupleVisibility::Invisible => {
                self.get_from_undo(tx_status, snapshot, owner, current.undo_head)
            }
        }
    }

    pub fn get_for_relation(
        &self,
        tx_status: &ConcurrentTxStatus,
        snapshot: &Snapshot,
        owner: Option<TxId>,
        rel_id: RelId,
        row_id: RowId,
    ) -> Result<Option<Vec<u8>>> {
        let Some(ptr) = self.head_for_relation(rel_id, row_id)? else {
            return Ok(None);
        };
        let current = self.read_tuple(ptr)?;
        if current.rel_id != rel_id {
            return Ok(None);
        }
        match current.visibility_concurrent(tx_status, snapshot, owner) {
            TupleVisibility::Visible => Ok(Some(current.payload)),
            TupleVisibility::Deleted => Ok(None),
            TupleVisibility::Invisible => {
                self.get_from_undo(tx_status, snapshot, owner, current.undo_head)
            }
        }
    }

    fn get_from_undo(
        &self,
        tx_status: &ConcurrentTxStatus,
        snapshot: &Snapshot,
        owner: Option<TxId>,
        undo_ptr: UndoPtr,
    ) -> Result<Option<Vec<u8>>> {
        let mut cursor = undo_ptr;
        let mut depth = 0_usize;
        while cursor != UndoPtr::ZERO {
            if let Some(limit) =
                ActiveUndoReadPolicy::depth_limit_hint(UndoReadContext { depth, ptr: cursor })
                && depth >= limit
            {
                return Ok(None);
            }
            let undo = self.read_undo(cursor)?;
            let tuple = TupleVersion::decode(&undo.before_image)?;
            match tuple.visibility_concurrent(tx_status, snapshot, owner) {
                TupleVisibility::Visible => return Ok(Some(tuple.payload)),
                TupleVisibility::Deleted => return Ok(None),
                TupleVisibility::Invisible => {
                    let next = undo.prev_undo;
                    let _ =
                        ActiveUndoReadPolicy::prefetch_next(UndoReadContext { depth, ptr: next });
                    cursor = next;
                    depth = depth.saturating_add(1);
                }
            }
        }
        Ok(None)
    }

    pub(super) fn visible_tuple_for_write(
        &self,
        tx_id: TxId,
        snapshot: &Snapshot,
        tx_status: &ConcurrentTxStatus,
        row_id: RowId,
    ) -> Result<TupleVersion> {
        let current = self.current_tuple_for_relation(self.rel_id, row_id)?;
        self.visible_current_tuple_for_write(tx_id, snapshot, tx_status, current)
    }

    pub(super) fn visible_tuple_for_write_in_relation(
        &self,
        tx_id: TxId,
        snapshot: &Snapshot,
        tx_status: &ConcurrentTxStatus,
        target: RelationWriteTarget,
    ) -> Result<TupleVersion> {
        let current = self.current_tuple_for_relation(target.rel_id, target.row_id)?;
        self.visible_current_tuple_for_write(tx_id, snapshot, tx_status, current)
    }

    pub(super) fn visible_current_tuple_for_write(
        &self,
        tx_id: TxId,
        snapshot: &Snapshot,
        tx_status: &ConcurrentTxStatus,
        current: TupleVersion,
    ) -> Result<TupleVersion> {
        match current.visibility_concurrent(tx_status, snapshot, Some(tx_id)) {
            TupleVisibility::Visible => return Ok(current),
            TupleVisibility::Deleted => return Err(Error::NotVisible),
            TupleVisibility::Invisible => {}
        }

        match tx_status.state(current.begin_tx) {
            TxState::Aborted => {
                let mut cursor = current.undo_head;
                let mut depth = 0_usize;
                while cursor != UndoPtr::ZERO {
                    if let Some(limit) = ActiveUndoReadPolicy::depth_limit_hint(UndoReadContext {
                        depth,
                        ptr: cursor,
                    }) && depth >= limit
                    {
                        return Err(Error::NotVisible);
                    }
                    let undo = self.read_undo(cursor)?;
                    let tuple = TupleVersion::decode(&undo.before_image)?;
                    match tuple.visibility_concurrent(tx_status, snapshot, Some(tx_id)) {
                        TupleVisibility::Visible => return Ok(tuple),
                        TupleVisibility::Deleted => return Err(Error::NotVisible),
                        TupleVisibility::Invisible => {
                            let next = undo.prev_undo;
                            let _ = ActiveUndoReadPolicy::prefetch_next(UndoReadContext {
                                depth,
                                ptr: next,
                            });
                            cursor = next;
                            depth = depth.saturating_add(1);
                        }
                    }
                }
                Err(Error::NotVisible)
            }
            TxState::Committed(_) | TxState::InProgress => Err(Error::SerializationFailure),
        }
    }

    #[allow(dead_code)]
    pub(super) fn current_tuple(&self, row_id: RowId) -> Result<TupleVersion> {
        self.current_tuple_for_relation(self.rel_id, row_id)
    }

    pub(crate) fn current_tuple_for_relation(
        &self,
        rel_id: RelId,
        row_id: RowId,
    ) -> Result<TupleVersion> {
        let rel_id = if rel_id == RelId::ZERO {
            self.rel_id
        } else {
            rel_id
        };
        let ptr = self
            .head_for_relation(rel_id, row_id)?
            .ok_or(Error::CorruptPage(
                "row id missing from relation row directory",
            ))?;
        self.read_tuple(ptr)
    }

    pub(crate) fn current_tuple_recovered(
        &self,
        rel_id: RelId,
        row_id: RowId,
    ) -> Result<TupleVersion> {
        if let Some(ptr) = self.head_for_relation(rel_id, row_id)? {
            return self.read_tuple(ptr);
        }
        let rel_id = if rel_id == RelId::ZERO {
            self.rel_id
        } else {
            rel_id
        };
        let page_count = self.page_count()?;
        for page_no in 1..=page_count {
            let page_id = PageId(page_no);
            let guard = match self.buffer.pin(page_id) {
                Ok(guard) => guard,
                Err(Error::InvalidMagic { actual: 0, .. }) => continue,
                Err(err) => return Err(err),
            };
            let current = guard.with_page(|page| {
                let header = page.header()?;
                if header.kind != PageKind::Heap || header.rel_id != self.rel_id {
                    return Ok(None);
                }
                for slot in 0..page.slot_count()? {
                    let tuple = TupleVersion::decode(page.cell(slot)?)?;
                    let tuple_rel_id = if tuple.rel_id == RelId::ZERO {
                        self.rel_id
                    } else {
                        tuple.rel_id
                    };
                    if tuple.row_id == row_id && tuple_rel_id == rel_id {
                        return Ok(Some(tuple));
                    }
                }
                Ok(None)
            })?;
            if let Some(current) = current {
                return Ok(current);
            }
        }
        Err(Error::CorruptPage(
            "row id missing from relation row directory",
        ))
    }

    pub(crate) fn read_tuple(&self, ptr: TuplePtr) -> Result<TupleVersion> {
        if ptr.is_null() {
            return Err(Error::CorruptPage("null tuple pointer"));
        }
        let guard = self.buffer.pin(ptr.page_id)?;
        guard.with_page(|page| {
            let header = page.header()?;
            if header.generation != ptr.generation {
                return Err(Error::CorruptPage(STALE_TUPLE_POINTER));
            }
            TupleVersion::decode(page.cell(ptr.slot)?)
        })
    }

    pub(crate) fn overwrite_tuple(&self, ptr: TuplePtr, tuple: &TupleVersion) -> Result<()> {
        if ptr.is_null() {
            return Err(Error::CorruptPage("null tuple pointer"));
        }
        let encoded = tuple.encode()?;
        let guard = self.buffer.pin(ptr.page_id)?;
        // Vacuum drops an undo link in place. The page still holds the live tuple, whose WAL
        // record is the page LSN already written there, so the page LSN stays as it is. One
        // latch covers the write and the dirty mark. With separate latches, an append could
        // install its page between them, and the older page LSN read first would replace the
        // append's.
        let mut frame = guard.mutable_frame()?;
        let page = frame
            .page
            .as_mut()
            .ok_or(Error::CorruptPage("resident frame missing page"))?;
        #[cfg(test)]
        super::test_hooks::run_before_tuple_overwrite_hook(page);
        // A page reinitialised for reuse since the caller read `ptr` holds
        // other tuples; the same slot may be another row's.
        if page.header()?.generation != ptr.generation {
            return Err(Error::CorruptPage(STALE_TUPLE_POINTER));
        }
        page.overwrite_cell(ptr.slot, &encoded)?;
        frame.dirty = true;
        Ok(())
    }

    pub(crate) fn read_undo(&self, ptr: UndoPtr) -> Result<UndoRecord> {
        if ptr == UndoPtr::ZERO {
            return Err(Error::CorruptPage("null undo pointer"));
        }
        let (page_id, slot) = decode_undo_ptr(ptr);
        let guard = self.buffer.pin(page_id)?;
        guard.with_page(|page| UndoRecord::decode(page.cell(slot)?))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::PageBackedHeap;
    use crate::format::{Lsn, RelId};
    use crate::storage::{BufferPool, PageFile};

    #[test]
    fn overwrite_keeps_the_page_lsn() {
        let dir = tempfile::tempdir().unwrap();
        let page_file = Arc::new(PageFile::create(dir.path().join("data.redline"), 4096).unwrap());
        let buffer = Arc::new(BufferPool::new(page_file, 8).unwrap());
        let heap = PageBackedHeap::new(RelId(1), 2, Arc::clone(&buffer)).unwrap();
        let row = heap.reserve_row_id();
        heap.insert_with_row_id(crate::format::TxId(1), row, b"row".to_vec(), Lsn(40))
            .unwrap();
        let ptr = heap.head(row).unwrap().unwrap();
        let tuple = heap.read_tuple(ptr).unwrap();
        heap.overwrite_tuple(ptr, &tuple).unwrap();
        let guard = buffer.pin(ptr.page_id).unwrap();
        let lsn = guard
            .with_page(|page| Ok(page.header().unwrap().page_lsn))
            .unwrap();
        assert_eq!(lsn, Lsn(40));
    }
}
