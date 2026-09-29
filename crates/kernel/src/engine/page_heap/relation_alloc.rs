//! Per-relation rowid allocation.
//!
//! Each relation hands out its own rowids, as each SQLite table does. A
//! relation's next rowid is one past the highest rowid it has held, and
//! only that relation's own deletes can lower it. With one engine-wide
//! counter, a DELETE in one table lowered the rowid every other table took
//! next, and an insert into another table then landed on a live row of its
//! own and replaced it.
//!
//! A relation's counter moves wherever the engine-wide one does: past every
//! row inserted, including an explicit rowid, past every row the WAL names
//! at recovery, and past every tuple the page-directory rebuild reads. A
//! delete's tombstone does not move it, so a delete of the highest rowid can
//! lower it first.

use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::format::{RelId, RowId};
use crate::{Error, Result};

use super::PageBackedHeap;
use super::policy::{ActiveHeapPlacementPolicy, HeapPlacementPolicy};

/// Next rowid per relation, sharded like the relation row directory.
pub(super) type RelationNextRow = Vec<RwLock<HashMap<RelId, AtomicU64>>>;

pub(super) fn new_relation_next_row(lanes: usize) -> RelationNextRow {
    (0..lanes).map(|_| RwLock::new(HashMap::new())).collect()
}

impl PageBackedHeap {
    /// Take the next rowid of `rel_id`.
    pub fn reserve_row_id_for(&self, rel_id: RelId) -> Result<RowId> {
        let rel_id = self.relation_or_default(rel_id);
        let shard = self.relation_next_row_shard(rel_id);
        {
            let map = shard.read().map_err(|_| poisoned())?;
            if let Some(next) = map.get(&rel_id) {
                return Ok(RowId(next.fetch_add(1, Ordering::SeqCst)));
            }
        }
        let mut map = shard.write().map_err(|_| poisoned())?;
        let next = map.entry(rel_id).or_insert_with(|| AtomicU64::new(1));
        Ok(RowId(next.fetch_add(1, Ordering::SeqCst)))
    }

    /// The rowid `rel_id` hands out next.
    pub fn relation_next_row(&self, rel_id: RelId) -> Result<u64> {
        let rel_id = self.relation_or_default(rel_id);
        let map = self
            .relation_next_row_shard(rel_id)
            .read()
            .map_err(|_| poisoned())?;
        Ok(map
            .get(&rel_id)
            .map_or(1, |next| next.load(Ordering::SeqCst)))
    }

    /// Lower the next rowid of `rel_id` from `expected` to `next_row`, as a
    /// delete of the relation's highest rowid does. Nothing changes, and
    /// `false` comes back, when the counter no longer holds `expected`: a
    /// rowid was taken meanwhile, and reusing a lower one is then unsafe.
    pub fn lower_relation_next_row(
        &self,
        rel_id: RelId,
        expected: u64,
        next_row: u64,
    ) -> Result<bool> {
        if next_row >= expected {
            return Ok(false);
        }
        let rel_id = self.relation_or_default(rel_id);
        let map = self
            .relation_next_row_shard(rel_id)
            .read()
            .map_err(|_| poisoned())?;
        Ok(map.get(&rel_id).is_some_and(|next| {
            next.compare_exchange(expected, next_row, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        }))
    }

    /// Keep `row_id` from being handed out again, by `rel_id` or by the
    /// engine-wide counter.
    pub(crate) fn reserve_recovered_row_id_in(&self, rel_id: RelId, row_id: RowId) -> Result<()> {
        self.reserve_recovered_row_id(row_id);
        self.advance_relation_next_row(rel_id, row_id)
    }

    /// Move the next rowid of `rel_id` past `row_id`.
    pub(crate) fn advance_relation_next_row(&self, rel_id: RelId, row_id: RowId) -> Result<()> {
        let rel_id = self.relation_or_default(rel_id);
        let past = row_id.0.saturating_add(1);
        let shard = self.relation_next_row_shard(rel_id);
        {
            let map = shard.read().map_err(|_| poisoned())?;
            if let Some(next) = map.get(&rel_id) {
                next.fetch_max(past, Ordering::SeqCst);
                return Ok(());
            }
        }
        let mut map = shard.write().map_err(|_| poisoned())?;
        map.entry(rel_id)
            .or_insert_with(|| AtomicU64::new(1))
            .fetch_max(past, Ordering::SeqCst);
        Ok(())
    }

    fn relation_or_default(&self, rel_id: RelId) -> RelId {
        if rel_id == RelId::ZERO {
            self.rel_id
        } else {
            rel_id
        }
    }

    fn relation_next_row_shard(&self, rel_id: RelId) -> &RwLock<HashMap<RelId, AtomicU64>> {
        let lane =
            ActiveHeapPlacementPolicy::relation_lane(rel_id, self.relation_next_row.len().max(1));
        &self.relation_next_row[lane]
    }
}

fn poisoned() -> Error {
    Error::CorruptPage("relation next-row shard poisoned")
}
