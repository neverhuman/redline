use crate::format::{Csn, PageId};
use crate::{Error, Result};

use super::super::{BtreeIndex, PAGE_LEAF_KIND};
use crate::engine::ConcurrentTxStatus;

impl BtreeIndex {
    pub fn compact_leaf_page(&self, page_id: PageId) -> Result<()> {
        let leaf_latch = self.inner.latches.get(page_id);
        let leaf_write = leaf_latch.write();
        let guard = self.inner.buffer.pin(page_id)?;
        let mut page = guard.mutable_frame()?;
        let page_ref = page
            .page
            .as_mut()
            .ok_or(Error::CorruptPage("resident frame missing page"))?;
        let header = Self::read_page_header(page_ref)?;
        if header.kind != PAGE_LEAF_KIND {
            return Err(Error::CorruptPage("expected leaf page"));
        }
        let entries = self.read_entries(page_ref)?;
        let live: Vec<_> = entries
            .into_iter()
            .filter(|entry| entry.physically_live())
            .collect();
        if live.len() == self.read_entries(page_ref)?.len() {
            return Ok(());
        }
        // Rewriting the leaf clears the header. Keep the LSN that already
        // covers the surviving keys so a checkpoint cannot treat this page
        // as older than those WAL records.
        let kept_lsn = page_ref.header()?.page_lsn;
        Self::rewrite_leaf(
            page_ref,
            self.descriptor().index_id,
            &live,
            header.left,
            header.right,
            header.high_key,
        )?;
        drop(page);
        guard.mark_dirty(kept_lsn)?;
        drop(leaf_write);
        Ok(())
    }

    pub fn prune_committed_deletes_before(
        &self,
        page_id: PageId,
        tx_status: &ConcurrentTxStatus,
        horizon: Csn,
    ) -> Result<()> {
        let leaf_latch = self.inner.latches.get(page_id);
        let leaf_write = leaf_latch.write();
        let guard = self.inner.buffer.pin(page_id)?;
        let mut page = guard.mutable_frame()?;
        let page_ref = page
            .page
            .as_mut()
            .ok_or(Error::CorruptPage("resident frame missing page"))?;
        let header = Self::read_page_header(page_ref)?;
        if header.kind != PAGE_LEAF_KIND {
            return Err(Error::CorruptPage("expected leaf page"));
        }
        let entries = self.read_entries(page_ref)?;
        let live: Vec<_> = entries
            .iter()
            .filter(|entry| !entry.is_committed_deleted_before(tx_status, horizon))
            .cloned()
            .collect();
        if live.len() == entries.len() {
            return Ok(());
        }
        let kept_lsn = page_ref.header()?.page_lsn;
        Self::rewrite_leaf(
            page_ref,
            self.descriptor().index_id,
            &live,
            header.left,
            header.right,
            header.high_key,
        )?;
        drop(page);
        guard.mark_dirty(kept_lsn)?;
        drop(leaf_write);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::format::{Lsn, PageGeneration, PageId, RelId, TuplePtr, TxId};
    use crate::index::{BtreeIndex, IndexDescriptor, IndexId, IndexRowRef, IndexUniqueness};
    use crate::storage::{BufferPool, PageFile};
    use crate::wal::{WalConfig, WalCoordinator};

    #[test]
    fn compact_keeps_the_wal_page_lsn() {
        let dir = tempfile::tempdir().unwrap();
        let page_file = Arc::new(PageFile::create(dir.path().join("data.redline"), 4096).unwrap());
        let buffer = Arc::new(BufferPool::new(Arc::clone(&page_file), 16).unwrap());
        let wal = WalCoordinator::create(
            dir.path().join("wal"),
            WalConfig {
                group_commit_delay_us: 0,
                ..WalConfig::default()
            },
        )
        .unwrap();
        let index = BtreeIndex::create_with_wal(
            Arc::clone(&buffer),
            IndexDescriptor::new(IndexId(4), RelId(1), IndexUniqueness::NonUnique),
            Some(Arc::new(wal)),
        )
        .unwrap();
        let root = index.meta().unwrap().root_page_id;
        let row_a = IndexRowRef::new(TuplePtr::new_with_generation(
            PageId(8),
            1,
            PageGeneration::ONE,
        ));
        let row_b = IndexRowRef::new(TuplePtr::new_with_generation(
            PageId(9),
            1,
            PageGeneration::ONE,
        ));
        index.insert_tx(TxId(1), b"k", row_a).unwrap();
        index.insert_tx(TxId(1), b"k", row_b).unwrap();
        index.delete_mark_tx(TxId(2), b"k", row_b).unwrap();
        let before = page_lsn(&buffer, root);
        assert!(before > Lsn(1), "wal insert lsn was {before:?}");
        index.compact_leaf_page(root).unwrap();
        assert_eq!(page_lsn(&buffer, root), before);
        assert_eq!(index.point_lookup(b"k").unwrap().len(), 1);
    }

    fn page_lsn(buffer: &BufferPool, page_id: PageId) -> Lsn {
        let guard = buffer.pin(page_id).unwrap();
        guard
            .with_page(|page| Ok(page.header().unwrap().page_lsn))
            .unwrap()
    }
}
