use crate::engine::ConcurrentTxStatus;
use crate::format::{Lsn, TxId};
use crate::wal::WalPayload;
use crate::{Error, Result};

use super::super::cells::Entry;
use super::super::{BtreeIndex, INDEX_SPECIAL_LEN, IndexRowRef, KeyBuf, PAGE_LEAF_KIND};
use super::versions::{CellFate, InsertView, cell_fate, next_version, same_entry};

impl BtreeIndex {
    pub fn insert(&self, logical_key: &[u8], row: IndexRowRef) -> Result<()> {
        self.insert_tx(crate::format::TxId::ZERO, logical_key, row)
    }

    pub fn insert_tx(
        &self,
        tx_id: crate::format::TxId,
        logical_key: &[u8],
        row: IndexRowRef,
    ) -> Result<()> {
        self.insert_tx_inner(
            tx_id,
            logical_key,
            row,
            InsertView::Plain,
            tx_id != TxId::ZERO,
            false,
            Lsn::new(1),
        )
    }

    /// Insert `(logical_key, row)` for `tx_id`, judging a cell the index
    /// already holds for the same key and row by commit state (see
    /// `mutate/versions.rs`). A row that left the index and comes back
    /// under the same key is live again once `tx_id` commits, and a
    /// snapshot that still reads the version it left keeps reading it.
    pub fn insert_tx_versioned(
        &self,
        tx_status: &ConcurrentTxStatus,
        tx_id: TxId,
        logical_key: &[u8],
        row: IndexRowRef,
    ) -> Result<()> {
        self.insert_tx_inner(
            tx_id,
            logical_key,
            row,
            InsertView::Versioned(tx_status),
            tx_id != TxId::ZERO,
            false,
            Lsn::new(1),
        )
    }

    pub(crate) fn insert_recovered_tx(
        &self,
        tx_id: crate::format::TxId,
        logical_key: &[u8],
        row: IndexRowRef,
        lsn: Lsn,
    ) -> Result<()> {
        self.insert_tx_inner(
            tx_id,
            logical_key,
            row,
            InsertView::Recovered,
            false,
            true,
            lsn,
        )
    }

    /// `recovered` marks a committed insert that recovery replays. A leaf
    /// it changes without splitting keeps a page LSN eviction may write
    /// (see `recovered_leaf_lsn`); a split it causes stamps `lsn` on every
    /// page it touches.
    #[allow(clippy::too_many_arguments)]
    fn insert_tx_inner(
        &self,
        tx_id: crate::format::TxId,
        logical_key: &[u8],
        mut row: IndexRowRef,
        view: InsertView<'_>,
        emit_wal: bool,
        recovered: bool,
        lsn: Lsn,
    ) -> Result<()> {
        let mut physical = KeyBuf::new();
        physical.extend_logical(logical_key);
        physical.append_row_ref_suffix(row);
        // Navigate by the candidate's *physical* bytes: separators in the tree
        // are normally logical keys, but a duplicate-key split (where every
        // entry on a leaf shared one logical key) installs a physical-key
        // separator instead. Comparing with physical bytes works for both
        // cases — physical = logical || row_ref_suffix, so it sorts identically
        // to logical for non-duplicate keys and resolves duplicates by row_id.
        loop {
            let root = self.meta()?.root_page_id;
            let path = self.find_leaf_path(root, physical.as_slice())?;
            let leaf_id = *path.last().ok_or(Error::CorruptPage("empty search path"))?;
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
            // Defensive: if the leaf still claims our key belongs to a sibling
            // (concurrent split window), retry the descent.
            if !header.high_key.is_empty() && physical.as_slice() >= header.high_key.as_slice() {
                drop(page);
                drop(leaf_write);
                continue;
            }
            let mut entries = self.read_entries(page_ref)?;
            let candidate = Entry::Leaf {
                logical_key: logical_key.to_vec(),
                row,
                physical: physical.as_slice().to_vec(),
                create_tx: tx_id,
                delete_tx: TxId::ZERO,
            };
            let found = entries.binary_search_by(|entry| entry.compare(&candidate));
            let attempt = row;
            // A cell of the same key and row: the candidate's own bytes, or
            // for a versioned insert a later version right after them.
            let mut at = match found {
                Ok(pos) => Some(pos),
                Err(pos) if matches!(view, InsertView::Versioned(_)) => {
                    entry_version_at(&entries, pos, logical_key, row)
                }
                Err(_) => None,
            };
            let mut restamp = None;
            while let Some(pos) = at {
                let Entry::Leaf {
                    row: cell_row,
                    create_tx,
                    delete_tx,
                    ..
                } = &entries[pos]
                else {
                    return Err(Error::CorruptPage("expected leaf cell"));
                };
                match cell_fate(view, tx_id, *create_tx, *delete_tx) {
                    CellFate::Keep => {
                        drop(page);
                        drop(leaf_write);
                        return Ok(());
                    }
                    CellFate::Restamp { create_tx } => {
                        restamp = Some((pos, create_tx));
                        break;
                    }
                    CellFate::Older => {
                        row = next_version(*cell_row)?;
                        at = entry_version_at(&entries, pos + 1, logical_key, row);
                    }
                }
            }
            if let Some((pos, create_tx)) = restamp {
                let Entry::Leaf {
                    row: cell_row,
                    create_tx: cell_create,
                    delete_tx: cell_delete,
                    ..
                } = &mut entries[pos]
                else {
                    return Err(Error::CorruptPage("expected leaf cell"));
                };
                *cell_create = create_tx;
                *cell_delete = TxId::ZERO;
                let cell_row = *cell_row;
                let unlogged_lsn = if recovered {
                    super::recovered_leaf_lsn(&page)?
                } else {
                    lsn
                };
                let page_ref = page
                    .page
                    .as_mut()
                    .ok_or(Error::CorruptPage("resident frame missing page"))?;
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
                // Replay restamps the same cell: recovery's insert of a
                // committed transaction makes an existing cell live.
                let publish_lsn = if emit_wal {
                    crate::fail_point!("index::insert");
                    self.append_index_delta(
                        tx_id,
                        WalPayload::IndexInsert {
                            tx_id,
                            index_id: self.descriptor().index_id.0,
                            logical_key: logical_key.to_vec(),
                            row: cell_row,
                        },
                    )?
                } else {
                    unlogged_lsn
                };
                #[cfg(test)]
                if emit_wal {
                    crate::wal::run_before_page_install_hook();
                }
                guard.install_dirty(staged, publish_lsn)?;
                drop(install_fence);
                drop(leaf_write);
                return Ok(());
            }
            if row != attempt {
                // The entry goes into a later version: find its place again.
                physical.clear();
                physical.extend_logical(logical_key);
                physical.append_row_ref_suffix(row);
                drop(page);
                drop(leaf_write);
                continue;
            }
            let slot = match found {
                Ok(_) => return Err(Error::CorruptPage("index insert slot taken")),
                Err(pos) => pos,
            };
            entries.insert(slot, candidate);
            let body_capacity = page_ref
                .as_bytes()
                .len()
                .saturating_sub(crate::format::PAGE_HEADER_LEN + INDEX_SPECIAL_LEN);
            let required =
                Self::encoded_entries_len(&entries) + entries.len() * crate::format::SLOT_LEN;
            if required > body_capacity {
                drop(page);
                drop(guard);
                drop(leaf_write);
                let _structure = self.lock_structure()?;
                let path = self.find_leaf_path(self.meta()?.root_page_id, physical.as_slice())?;
                let leaf_id = *path.last().ok_or(Error::CorruptPage("empty search path"))?;
                self.split_leaf_and_insert(
                    &path[..path.len().saturating_sub(1)],
                    leaf_id,
                    logical_key,
                    row,
                    physical.as_slice().to_vec(),
                    tx_id,
                    emit_wal,
                    lsn,
                )?;
                return Ok(());
            }
            let unlogged_lsn = if recovered {
                super::recovered_leaf_lsn(&page)?
            } else {
                lsn
            };
            // Stage off the live frame. A flush during the WAL append still
            // sees the previous leaf bytes and the previous page LSN.
            let page_ref = page
                .page
                .as_mut()
                .ok_or(Error::CorruptPage("resident frame missing page"))?;
            let mut staged = page_ref.clone();
            if super::insert_leaf::direct_leaf_insert_enabled() {
                super::insert_leaf::insert_one_cell(&mut staged, slot, &entries[slot])?;
            } else {
                Self::rewrite_leaf(
                    &mut staged,
                    self.descriptor().index_id,
                    &entries,
                    header.left,
                    header.right,
                    header.high_key,
                )?;
            }
            drop(page);
            // The WAL record can become durable before install_dirty publishes
            // the leaf. Hold the checkpoint horizon across that gap.
            let install_fence = match &self.inner.wal {
                Some(wal) if emit_wal => Some(wal.begin_page_install()?),
                _ => None,
            };
            let publish_lsn = if emit_wal {
                // Armed before the staged leaf is installed. A crash here
                // leaves the on-buffer leaf unchanged, so the key is absent.
                crate::fail_point!("index::insert");
                self.append_index_delta(
                    tx_id,
                    WalPayload::IndexInsert {
                        tx_id,
                        index_id: self.descriptor().index_id.0,
                        logical_key: logical_key.to_vec(),
                        row,
                    },
                )?
            } else {
                unlogged_lsn
            };
            #[cfg(test)]
            if emit_wal {
                crate::wal::run_before_page_install_hook();
            }
            guard.install_dirty(staged, publish_lsn)?;
            drop(install_fence);
            drop(leaf_write);
            return Ok(());
        }
    }

    pub fn insert_unique(&self, owner: u64, logical_key: &[u8], row: IndexRowRef) -> Result<()> {
        let _guard = self.lock_unique_key(owner, logical_key)?;
        if !self.point_lookup(logical_key)?.is_empty() {
            return Err(Error::WriteConflict);
        }
        self.insert(logical_key, row)
    }
}

/// The index of the cell at `pos` when it holds a version of the entry
/// `(logical_key, row)`: same key and row, any tuple generation.
fn entry_version_at(
    entries: &[Entry],
    pos: usize,
    logical_key: &[u8],
    row: IndexRowRef,
) -> Option<usize> {
    match entries.get(pos)? {
        Entry::Leaf {
            logical_key: key,
            row: cell_row,
            ..
        } if key.as_slice() == logical_key && same_entry(*cell_row, row) => Some(pos),
        _ => None,
    }
}
