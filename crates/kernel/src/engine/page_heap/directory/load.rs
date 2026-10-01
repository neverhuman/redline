use std::collections::HashSet;

use crate::engine::tx::ConcurrentTxStatus;
use crate::format::{PageId, PageKind, PageState, RelId, RowId, TuplePtr, TupleVersion};
use crate::txn::Snapshot;
use crate::{Error, Result};

use super::super::{PageBackedHeap, advance_atomic_past};
use super::load_heads::HeadChoices;

impl PageBackedHeap {
    /// Rebuild the row directory from the heap pages for every row that has
    /// no head yet. `txs` must already hold every recovered commit: it
    /// decides which on-disk version of a row is the newest.
    pub fn load_row_directory_from_pages(
        &self,
        page_count: u64,
        txs: &ConcurrentTxStatus,
    ) -> Result<()> {
        let mut choices = HeadChoices::default();
        for page_no in 1..=page_count {
            let page_id = PageId(page_no);
            let guard = match self.buffer.pin(page_id) {
                Ok(guard) => guard,
                Err(Error::InvalidMagic { actual: 0, .. }) => continue,
                Err(err) => return Err(err),
            };
            guard.with_page(|page| {
                let header = page.header()?;
                if header.kind != PageKind::Heap || header.rel_id != self.rel_id {
                    return Ok(());
                }
                if header.state == PageState::Reusable {
                    return Ok(());
                }

                for slot in 0..page.slot_count()? {
                    let tuple = TupleVersion::decode(page.cell(slot)?)?;
                    advance_atomic_past(&self.next_row, tuple.row_id.0);
                    let rel_id = if tuple.rel_id == RelId::ZERO {
                        self.rel_id
                    } else {
                        tuple.rel_id
                    };
                    self.advance_relation_next_row(rel_id, tuple.row_id)?;
                    let ptr = TuplePtr::new_with_generation(page_id, slot, header.generation);
                    choices.offer(txs, rel_id, &tuple, ptr);
                }
                Ok(())
            })?;
        }
        let heads = choices.into_heads(
            |ptr| self.read_tuple(ptr).ok(),
            |undo| self.read_undo(undo).ok(),
        );
        for (rel_id, row_id, ptr) in heads {
            if self.head(row_id)?.is_none() {
                self.set_head(row_id, ptr)?;
            }
            if self.head_for_relation(rel_id, row_id)?.is_none() {
                self.set_relation_head(rel_id, row_id, ptr)?;
            }
        }
        Ok(())
    }

    pub fn invalidate_row_directory_for_pages(&self, pages: &[PageId]) -> Result<()> {
        let pages: HashSet<PageId> = pages.iter().copied().collect();
        for shard in &self.row_dir {
            let mut shard = shard
                .write()
                .map_err(|_| Error::CorruptPage("row directory shard poisoned"))?;
            shard.retain(|_, ptr| !pages.contains(&ptr.page_id));
        }
        for shard in &self.relation_row_dir {
            let mut shard = shard
                .write()
                .map_err(|_| Error::CorruptPage("relation row directory shard poisoned"))?;
            shard.retain(|_, rows| {
                rows.retain(|_, ptr| !pages.contains(&ptr.page_id));
                !rows.is_empty()
            });
        }
        Ok(())
    }

    pub fn load_reusable_pages_from_pages(&self, page_count: u64) -> Result<()> {
        for page_no in 1..=page_count {
            let page_id = PageId(page_no);
            let guard = match self.buffer.pin(page_id) {
                Ok(guard) => guard,
                Err(Error::InvalidMagic { actual: 0, .. }) => continue,
                Err(err) => return Err(err),
            };
            let header = guard.with_page(|page| page.header())?;
            if header.rel_id != self.rel_id || header.state != PageState::Reusable {
                continue;
            }
            self.push_reusable_page(header.kind, page_id)?;
        }
        Ok(())
    }

    pub fn row_directory_entries(&self) -> Result<Vec<(RowId, TuplePtr)>> {
        let mut rows = Vec::new();
        for shard in &self.row_dir {
            let shard = shard
                .read()
                .map_err(|_| Error::CorruptPage("row dir shard poisoned"))?;
            rows.extend(shard.iter().map(|(row_id, ptr)| (*row_id, *ptr)));
        }
        Ok(rows)
    }

    pub fn relation_entries(&self, rel_id: RelId) -> Result<Vec<(RowId, TuplePtr)>> {
        let mut rows = Vec::new();
        for shard in &self.relation_row_dir {
            let shard = shard
                .read()
                .map_err(|_| Error::CorruptPage("relation row dir shard poisoned"))?;
            if let Some(entries) = shard.get(&rel_id) {
                crate::observe::add_directory_entries_copied(entries.len() as u64);
                rows.extend(entries.iter().map(|(row_id, ptr)| (*row_id, *ptr)));
            }
        }
        Ok(rows)
    }

    /// Walk the relation's row directory and return every row that is
    /// visible to `snapshot`. Lane INT consumes this for the heap/index
    /// equivalence check: every row returned here MUST resolve to an index
    /// entry on every catalog index over the relation. Rows that are
    /// invisible (uncommitted or shadowed by a later version), tombstoned,
    /// or whose head has been concurrently vacuumed are silently skipped —
    /// they are not part of the durable visible state.
    pub fn iter_visible_rows_for_relation(
        &self,
        tx_status: &ConcurrentTxStatus,
        snapshot: &Snapshot,
        rel_id: RelId,
    ) -> Result<Vec<(RowId, Vec<u8>)>> {
        crate::fail_point!("integrity::heap::visible");
        let entries = self.relation_entries(rel_id)?;
        let mut out = Vec::with_capacity(entries.len());
        for (row_id, _ptr) in entries {
            let payload = self.get_for_relation(tx_status, snapshot, None, rel_id, row_id)?;
            if let Some(payload) = payload {
                out.push((row_id, payload));
            }
        }
        Ok(out)
    }

    pub fn relation_rowids(&self, rel_id: RelId) -> Result<Vec<RowId>> {
        let mut rows = Vec::new();
        for shard in &self.relation_row_dir {
            let shard = shard
                .read()
                .map_err(|_| Error::CorruptPage("relation row dir shard poisoned"))?;
            if let Some(entries) = shard.get(&rel_id) {
                crate::observe::add_directory_entries_copied(entries.len() as u64);
                rows.extend(entries.keys().copied());
            }
        }
        Ok(rows)
    }
}
