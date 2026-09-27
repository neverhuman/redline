//! Choose each row's head when the row directory is rebuilt from pages.
//!
//! A row can have several versions on disk: every update and delete appends
//! one, and the old ones stay until vacuum removes them. The version that
//! was appended last is the head. Tuples do not record when they were
//! appended, so the choice uses what the pages do show:
//!
//! * A committed version outranks every version whose transaction did not
//!   commit. Recovery treats those as aborted, and their undo records need
//!   not have reached the page file.
//! * Between committed versions, the later commit wins.
//! * Versions of one transaction tie. A version whose undo before-image is
//!   another tied version replaced it. Otherwise the later position wins,
//!   pages in ascending order and slots in ascending order within a page.
//!
//! The last rule can still pick the wrong version when one transaction
//! wrote two versions of a row without an undo link between them, such as a
//! delete followed by a new insert of the same row id, and the later one
//! landed on a reused page with a lower id.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::engine::tx::ConcurrentTxStatus;
use crate::format::{Csn, RelId, RowId, TuplePtr, TupleVersion, TxId, UndoPtr};
use crate::txn::{TxState, UndoRecord};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    Uncommitted,
    Committed(Csn),
}

/// What identifies a version inside its row's undo chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    begin_tx: TxId,
    undo_head: UndoPtr,
    flags: u16,
    payload_hash: u64,
}

impl Identity {
    fn of(tuple: &TupleVersion) -> Self {
        let mut hasher = DefaultHasher::new();
        tuple.payload.hash(&mut hasher);
        Self {
            begin_tx: tuple.begin_tx,
            undo_head: tuple.undo_head,
            flags: tuple.flags,
            payload_hash: hasher.finish(),
        }
    }
}

/// Head candidates collected over a page scan. Only tuple pointers are
/// kept, so the rebuild stays close to the size of the directory it builds;
/// tied versions are read back when their tie is broken.
#[derive(Debug, Default)]
pub(super) struct HeadChoices {
    rows: HashMap<(RelId, RowId), Choice>,
}

/// The best rank seen for a row and the versions that share it, in scan
/// order. Most rows have one, which needs no allocation.
#[derive(Debug)]
struct Choice {
    rank: Rank,
    first: TuplePtr,
    ties: Vec<TuplePtr>,
}

impl HeadChoices {
    /// Offer a version found at `ptr`. Call in scan order.
    pub(super) fn offer(
        &mut self,
        txs: &ConcurrentTxStatus,
        rel_id: RelId,
        tuple: &TupleVersion,
        ptr: TuplePtr,
    ) {
        let rank = match txs.state(tuple.begin_tx) {
            TxState::Committed(csn) => Rank::Committed(csn),
            TxState::InProgress | TxState::Aborted => Rank::Uncommitted,
        };
        let fresh = Choice {
            rank,
            first: ptr,
            ties: Vec::new(),
        };
        match self.rows.entry((rel_id, tuple.row_id)) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(fresh);
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                let choice = slot.get_mut();
                if rank > choice.rank {
                    *choice = fresh;
                } else if rank == choice.rank {
                    choice.ties.push(ptr);
                }
            }
        }
    }

    /// The chosen head of every row. `read_tuple` reads a tied version back
    /// and `read_undo` the undo record it points at; one either cannot read
    /// breaks no tie.
    pub(super) fn into_heads(
        self,
        mut read_tuple: impl FnMut(TuplePtr) -> Option<TupleVersion>,
        mut read_undo: impl FnMut(UndoPtr) -> Option<UndoRecord>,
    ) -> Vec<(RelId, RowId, TuplePtr)> {
        let mut heads = Vec::with_capacity(self.rows.len());
        for ((rel_id, row_id), choice) in self.rows {
            let ptr = if choice.ties.is_empty() {
                choice.first
            } else {
                let mut tied = Vec::with_capacity(choice.ties.len() + 1);
                tied.push(choice.first);
                tied.extend(choice.ties);
                newest(&tied, &mut read_tuple, &mut read_undo)
            };
            heads.push((rel_id, row_id, ptr));
        }
        heads
    }
}

/// The newest of several versions that share a rank, given in scan order.
fn newest(
    tied: &[TuplePtr],
    read_tuple: &mut impl FnMut(TuplePtr) -> Option<TupleVersion>,
    read_undo: &mut impl FnMut(UndoPtr) -> Option<UndoRecord>,
) -> TuplePtr {
    let tuples: Vec<Option<TupleVersion>> = tied.iter().map(|ptr| read_tuple(*ptr)).collect();
    let identities: Vec<Option<Identity>> = tuples
        .iter()
        .map(|t| t.as_ref().map(Identity::of))
        .collect();
    let mut replaced = vec![false; tied.len()];
    for (index, tuple) in tuples.iter().enumerate() {
        let Some(tuple) = tuple else {
            continue;
        };
        if tuple.undo_head == UndoPtr::ZERO {
            continue;
        }
        let Some(undo) = read_undo(tuple.undo_head) else {
            continue;
        };
        let Ok(before) = TupleVersion::decode(&undo.before_image) else {
            continue;
        };
        let before = Identity::of(&before);
        for (other, older) in identities.iter().enumerate() {
            if other != index && *older == Some(before) {
                replaced[other] = true;
            }
        }
    }
    tied.iter()
        .zip(&replaced)
        .rev()
        .find(|(_, replaced)| !**replaced)
        .map_or(tied[tied.len() - 1], |(ptr, _)| *ptr)
}
