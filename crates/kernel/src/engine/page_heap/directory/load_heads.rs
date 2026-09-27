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

#[derive(Debug)]
struct Candidate {
    ptr: TuplePtr,
    identity: Identity,
}

/// Head candidates collected over a page scan.
#[derive(Debug, Default)]
pub(super) struct HeadChoices {
    rows: HashMap<(RelId, RowId), (Rank, Vec<Candidate>)>,
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
        let candidate = Candidate {
            ptr,
            identity: Identity::of(tuple),
        };
        let entry = self
            .rows
            .entry((rel_id, tuple.row_id))
            .or_insert_with(|| (rank, Vec::new()));
        if rank > entry.0 {
            *entry = (rank, Vec::new());
        }
        if rank == entry.0 {
            entry.1.push(candidate);
        }
    }

    /// The chosen head of every row. `read_undo` loads the undo record a
    /// tied version points at; a record it cannot read breaks no tie.
    pub(super) fn into_heads(
        self,
        mut read_undo: impl FnMut(UndoPtr) -> Option<UndoRecord>,
    ) -> Vec<(RelId, RowId, TuplePtr)> {
        let mut heads = Vec::with_capacity(self.rows.len());
        for ((rel_id, row_id), (_, candidates)) in self.rows {
            if let Some(ptr) = newest(&candidates, &mut read_undo) {
                heads.push((rel_id, row_id, ptr));
            }
        }
        heads
    }
}

fn newest(
    candidates: &[Candidate],
    read_undo: &mut impl FnMut(UndoPtr) -> Option<UndoRecord>,
) -> Option<TuplePtr> {
    if candidates.len() <= 1 {
        return candidates.first().map(|candidate| candidate.ptr);
    }
    let mut replaced = vec![false; candidates.len()];
    for (index, candidate) in candidates.iter().enumerate() {
        if candidate.identity.undo_head == UndoPtr::ZERO {
            continue;
        }
        let Some(undo) = read_undo(candidate.identity.undo_head) else {
            continue;
        };
        let Ok(before) = TupleVersion::decode(&undo.before_image) else {
            continue;
        };
        let before = Identity::of(&before);
        for (other, older) in candidates.iter().enumerate() {
            if other != index && older.identity == before {
                replaced[other] = true;
            }
        }
    }
    candidates
        .iter()
        .zip(&replaced)
        .rev()
        .find(|(_, replaced)| !**replaced)
        .or_else(|| candidates.iter().zip(&replaced).next_back())
        .map(|(candidate, _)| candidate.ptr)
}
