use crate::format::{Lsn, TxId};
use crate::txn::Snapshot;
use crate::{Error, Result};

use super::super::cells::delete_marker_visible;
use super::super::{BtreeIndex, IndexRowRef, KeyBuf, NON_TRANSACTIONAL_DELETE_TX, PAGE_LEAF_KIND};
use crate::engine::ConcurrentTxStatus;
use crate::wal::WalPayload;

impl BtreeIndex {
    pub fn delete_mark(&self, logical_key: &[u8], row: IndexRowRef) -> Result<()> {
        self.delete_mark_tx(NON_TRANSACTIONAL_DELETE_TX, logical_key, row)
    }

    pub fn delete_mark_tx(
        &self,
        tx_id: crate::format::TxId,
        logical_key: &[u8],
        row: IndexRowRef,
    ) -> Result<()> {
        self.delete_mark_tx_inner(
            tx_id,
            logical_key,
            row,
            None,
            tx_id != TxId::ZERO,
            Lsn::new(1),
        )
    }

    pub(crate) fn delete_mark_recovered_tx(
        &self,
        tx_id: crate::format::TxId,
        logical_key: &[u8],
        row: IndexRowRef,
        lsn: Lsn,
    ) -> Result<()> {
        self.delete_mark_tx_inner(tx_id, logical_key, row, None, false, lsn)
    }

    pub fn delete_mark_tx_visible(
        &self,
        tx_status: &ConcurrentTxStatus,
        snapshot: &Snapshot,
        owner: Option<TxId>,
        tx_id: TxId,
        logical_key: &[u8],
        row: IndexRowRef,
    ) -> Result<()> {
        self.delete_mark_tx_inner(
            tx_id,
            logical_key,
            row,
            Some((tx_status, snapshot, owner)),
            tx_id != TxId::ZERO,
            Lsn::new(1),
        )
    }

    fn delete_mark_tx_inner(
        &self,
        tx_id: TxId,
        logical_key: &[u8],
        row: IndexRowRef,
        visibility: Option<(&ConcurrentTxStatus, &Snapshot, Option<TxId>)>,
        emit_wal: bool,
        lsn: Lsn,
    ) -> Result<()> {
        let mut probe = KeyBuf::new();
        probe.extend_logical(logical_key);
        probe.append_row_ref_suffix(row);
        let path = self.find_leaf_path(self.meta()?.root_page_id, probe.as_slice())?;
        let mut leaf_id = *path.last().ok_or(Error::CorruptPage("empty search path"))?;
        loop {
            let leaf_latch = self.inner.latches.get(leaf_id);
            let leaf_write = leaf_latch.write();
            let guard = self.inner.buffer.pin(leaf_id)?;
            let mut page = guard.mutable_frame()?;
            let page_ref = page
                .page
                .as_mut()
                .ok_or(Error::CorruptPage("resident frame missing page"))?;
            let header = Self::read_page_header(page_ref)?;
            if header.kind != PAGE_LEAF_KIND {
                return Err(Error::CorruptPage("expected leaf page"));
            }
            if !header.high_key.is_empty() && probe.as_slice() >= header.high_key.as_slice() {
                let Some(right) = header.right else {
                    return Err(Error::CorruptPage("index high key missing right link"));
                };
                drop(page);
                drop(leaf_write);
                leaf_id = right;
                continue;
            }
            let mut entries = self.read_entries(page_ref)?;
            let mut changed = false;
            let mut last_key_matches_search = false;
            for entry in &mut entries {
                if let crate::index::cells::Entry::Leaf {
                    logical_key: key,
                    row: entry_row,
                    delete_tx,
                    ..
                } = entry
                {
                    if key.as_slice() == logical_key {
                        last_key_matches_search = true;
                    }
                    if key.as_slice() == logical_key && *entry_row == row {
                        if delete_marker_visible(*delete_tx, visibility) {
                            return Ok(());
                        }
                        *delete_tx = tx_id;
                        changed = true;
                        break;
                    }
                }
            }
            if changed {
                let mut staged = page_ref.clone();
                Self::rewrite_leaf(
                    &mut staged,
                    self.descriptor().index_id,
                    &entries,
                    header.left,
                    header.right,
                    header.high_key,
                )?;
                drop(page);
                let install_fence = match &self.inner.wal {
                    Some(wal) if emit_wal => Some(wal.begin_page_install()?),
                    _ => None,
                };
                // Armed before the delete mark is installed. A crash here
                // leaves the live leaf without the tombstone.
                let publish_lsn = if emit_wal {
                    crate::fail_point!("index::delete");
                    let end_lsn = self.append_index_delta(
                        tx_id,
                        WalPayload::IndexDelete {
                            tx_id,
                            index_id: self.descriptor().index_id.0,
                            logical_key: logical_key.to_vec(),
                            row,
                        },
                    )?;
                    #[cfg(test)]
                    crate::wal::run_before_page_install_hook();
                    end_lsn
                } else {
                    lsn
                };
                guard.install_dirty(staged, publish_lsn)?;
                drop(install_fence);
                drop(leaf_write);
                return Ok(());
            }
            // No match here. If duplicates of this logical_key may continue on
            // the right sibling, walk right and keep looking.
            drop(page);
            if last_key_matches_search && let Some(right) = header.right {
                drop(leaf_write);
                leaf_id = right;
                continue;
            }
            drop(leaf_write);
            return Ok(());
        }
    }
}
