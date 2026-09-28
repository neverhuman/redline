//! Versions of one index entry: what an insert or a delete mark does when the
//! leaf already holds cells for the same logical key and row.
//!
//! An entry's physical key is `logical key || row reference`. A row that
//! leaves an index and later comes back under the same key and row -- a
//! partial-index flag toggled 1 -> 0 -> 1, a key moved A -> B -> A, a
//! DELETE followed by an INSERT of the same rowid -- therefore meets the cell
//! it left behind. The insert used to see that cell and return, so the row
//! stayed out of the index for good.
//!
//! The cell stamps (`create_tx`, `delete_tx`) say who can still read it.
//! When the transaction that removed the row committed, an older snapshot may
//! still read the cell, so the insert keeps it and adds the next version: the
//! same key and row with the row reference's tuple generation one higher.
//! The versions sort next to each other, and a snapshot sees at most one of
//! them. When no snapshot can read the cell any more, or the inserting
//! transaction removed it itself, the insert restamps it in place.
//!
//! Deciding by commit state rather than by the inserter's snapshot is
//! deliberate: an autocommit write runs read-committed and changes the latest
//! row version, which can be newer than its snapshot.

use crate::engine::ConcurrentTxStatus;
use crate::format::{PageGeneration, TxId};
use crate::txn::TxState;
use crate::{Error, Result};

use super::super::{IndexRowRef, NON_TRANSACTIONAL_DELETE_TX};

/// How an insert judges a cell that already holds its key and row.
#[derive(Clone, Copy)]
pub(super) enum InsertView<'a> {
    /// `insert_tx`: no commit states at hand. Only a cell this transaction
    /// removed, or one removed outside any transaction, is restamped; any
    /// other existing cell is taken as the entry, as before.
    Plain,
    /// Recovery replays the insert of a committed transaction. Every
    /// transaction recovery replays is committed, so after recovery the
    /// cell needs only to be live.
    Recovered,
    /// SQL DML: judge the cell by commit state.
    Versioned(&'a ConcurrentTxStatus),
}

/// What an insert does with an existing cell of its key and row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CellFate {
    /// The cell already is the live entry: nothing to do.
    Keep,
    /// Rewrite the cell's stamps to `(create_tx, live)`.
    Restamp { create_tx: TxId },
    /// The cell is a removed version an older snapshot may still read: keep
    /// it and put the entry in the next version.
    Older,
}

pub(super) fn cell_fate(
    view: InsertView<'_>,
    tx_id: TxId,
    create_tx: TxId,
    delete_tx: TxId,
) -> CellFate {
    match view {
        InsertView::Plain => {
            if delete_tx == TxId::ZERO {
                CellFate::Keep
            } else if delete_tx == tx_id {
                CellFate::Restamp { create_tx }
            } else if delete_tx == NON_TRANSACTIONAL_DELETE_TX {
                CellFate::Restamp { create_tx: tx_id }
            } else {
                CellFate::Keep
            }
        }
        InsertView::Recovered => {
            if create_tx == tx_id && delete_tx == TxId::ZERO {
                CellFate::Keep
            } else {
                CellFate::Restamp { create_tx: tx_id }
            }
        }
        InsertView::Versioned(status) => versioned_fate(status, tx_id, create_tx, delete_tx),
    }
}

fn versioned_fate(
    status: &ConcurrentTxStatus,
    tx_id: TxId,
    create_tx: TxId,
    delete_tx: TxId,
) -> CellFate {
    let created = match create_tx {
        TxId::ZERO => Created::Live,
        tx if tx == tx_id => Created::Live,
        tx => match status.state(tx) {
            TxState::Committed(_) => Created::Live,
            TxState::Aborted => Created::Aborted,
            TxState::InProgress => Created::Pending,
        },
    };
    match created {
        // Nobody reads the cell of a rolled-back insert.
        Created::Aborted => return CellFate::Restamp { create_tx: tx_id },
        // Another writer's cell: not ours to take over.
        Created::Pending => return CellFate::Keep,
        Created::Live => {}
    }
    if delete_tx == TxId::ZERO {
        return CellFate::Keep;
    }
    if delete_tx == tx_id {
        return CellFate::Restamp { create_tx };
    }
    if delete_tx == NON_TRANSACTIONAL_DELETE_TX {
        return CellFate::Restamp { create_tx: tx_id };
    }
    match status.state(delete_tx) {
        TxState::Committed(_) => CellFate::Older,
        // A rolled-back or unfinished removal leaves the cell live.
        TxState::Aborted | TxState::InProgress => CellFate::Keep,
    }
}

enum Created {
    Live,
    Aborted,
    Pending,
}

/// Whether a delete mark may take this cell: the version the latest state
/// holds. A cell whose removal committed, or that this transaction or a
/// non-transactional delete already removed, is an older version; the cell
/// of a rolled-back insert is no version at all.
pub(super) fn deletable_version(
    status: &ConcurrentTxStatus,
    tx_id: TxId,
    create_tx: TxId,
    delete_tx: TxId,
) -> bool {
    if create_tx != TxId::ZERO
        && create_tx != tx_id
        && matches!(status.state(create_tx), TxState::Aborted)
    {
        return false;
    }
    match delete_tx {
        TxId::ZERO => true,
        tx if tx == tx_id || tx == NON_TRANSACTIONAL_DELETE_TX => false,
        tx => !matches!(status.state(tx), TxState::Committed(_)),
    }
}

/// `a` and `b` name the same row as versions of one entry: everything but
/// the tuple generation agrees.
pub(super) fn same_entry(a: IndexRowRef, b: IndexRowRef) -> bool {
    a.row_id == b.row_id && a.tuple.page_id == b.tuple.page_id && a.tuple.slot == b.tuple.slot
}

/// The row reference of the version after `row`.
pub(super) fn next_version(row: IndexRowRef) -> Result<IndexRowRef> {
    let generation = row
        .tuple
        .generation
        .0
        .checked_add(1)
        .ok_or(Error::CorruptPage("index entry version counter exhausted"))?;
    let mut next = row;
    next.tuple.generation = PageGeneration::new(generation);
    Ok(next)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::engine::ConcurrentTxStatus;
    use crate::format::{PageGeneration, PageId, RelId, RowId, TuplePtr, TxId};
    use crate::index::{BtreeIndex, IndexDescriptor, IndexId, IndexRowRef, IndexUniqueness};
    use crate::storage::{BufferPool, PageFile};
    use crate::txn::Snapshot;

    use super::{CellFate, InsertView, cell_fate, deletable_version};

    fn row(id: u64) -> IndexRowRef {
        IndexRowRef::with_row_id(
            RowId(id),
            TuplePtr::new_with_generation(PageId(0), 0, PageGeneration::ONE),
        )
    }

    fn fresh(page_size: usize) -> (tempfile::TempDir, BtreeIndex) {
        let dir = tempfile::tempdir().expect("temp dir");
        let page_file =
            Arc::new(PageFile::create(dir.path().join("data.redline"), page_size).expect("file"));
        let buffer = Arc::new(BufferPool::new(page_file, 256).expect("pool"));
        let index = BtreeIndex::create(
            buffer,
            IndexDescriptor::new(IndexId(7), RelId(1), IndexUniqueness::NonUnique),
        )
        .expect("index");
        (dir, index)
    }

    fn commit(status: &ConcurrentTxStatus, tx: TxId) {
        let csn = status.reserve_commit_csn();
        status.publish_commit(tx, csn);
    }

    fn ids(
        index: &BtreeIndex,
        status: &ConcurrentTxStatus,
        snapshot: &Snapshot,
        key: &[u8],
    ) -> Vec<u64> {
        let mut out: Vec<u64> = index
            .point_lookup_visible(status, snapshot, None, key)
            .expect("lookup")
            .iter()
            .map(|row| row.row_id.0)
            .collect();
        out.sort_unstable();
        out
    }

    fn insert(index: &BtreeIndex, status: &ConcurrentTxStatus, key: &[u8], id: u64) -> TxId {
        let tx = status.begin();
        index
            .insert_tx_versioned(status, tx, key, row(id))
            .expect("insert");
        tx
    }

    fn delete(index: &BtreeIndex, status: &ConcurrentTxStatus, key: &[u8], id: u64) -> TxId {
        let tx = status.begin();
        let snapshot = status.snapshot();
        index
            .delete_mark_tx_visible(status, &snapshot, Some(tx), tx, key, row(id))
            .expect("delete");
        tx
    }

    #[test]
    fn fate_follows_commit_state() {
        let status = ConcurrentTxStatus::new();
        let committed = status.begin();
        commit(&status, committed);
        let aborted = status.begin();
        status.abort(aborted);
        let running = status.begin();
        let me = status.begin();
        let view = InsertView::Versioned(&status);
        let fate = |create, delete| cell_fate(view, me, create, delete);
        assert_eq!(fate(committed, TxId::ZERO), CellFate::Keep);
        assert_eq!(fate(TxId::ZERO, TxId::ZERO), CellFate::Keep);
        assert_eq!(fate(me, TxId::ZERO), CellFate::Keep);
        assert_eq!(
            fate(committed, me),
            CellFate::Restamp {
                create_tx: committed
            }
        );
        assert_eq!(
            fate(aborted, TxId::ZERO),
            CellFate::Restamp { create_tx: me }
        );
        assert_eq!(fate(aborted, me), CellFate::Restamp { create_tx: me });
        assert_eq!(fate(running, TxId::ZERO), CellFate::Keep);
        assert_eq!(fate(committed, aborted), CellFate::Keep);
        assert_eq!(fate(committed, running), CellFate::Keep);
        assert_eq!(fate(committed, committed), CellFate::Older);
        assert_eq!(
            fate(committed, super::NON_TRANSACTIONAL_DELETE_TX),
            CellFate::Restamp { create_tx: me }
        );
        assert_eq!(
            cell_fate(InsertView::Recovered, me, committed, committed),
            CellFate::Restamp { create_tx: me }
        );
        assert_eq!(
            cell_fate(InsertView::Recovered, me, me, TxId::ZERO),
            CellFate::Keep
        );
        assert_eq!(
            cell_fate(InsertView::Plain, me, committed, committed),
            CellFate::Keep
        );
        assert!(deletable_version(&status, me, committed, TxId::ZERO));
        assert!(deletable_version(&status, me, committed, aborted));
        assert!(!deletable_version(&status, me, committed, committed));
        assert!(!deletable_version(&status, me, committed, me));
        assert!(!deletable_version(&status, me, aborted, TxId::ZERO));
    }

    /// Out and back in, many times, with a snapshot kept from every state:
    /// each snapshot sees exactly the membership it began with.
    #[test]
    fn every_snapshot_sees_its_own_version() {
        // Small pages so the versions of one entry span several leaves.
        let (_dir, index) = fresh(512);
        let status = ConcurrentTxStatus::new();
        for id in 1..=40 {
            let tx = insert(&index, &status, format!("k{id:03}").as_bytes(), id);
            commit(&status, tx);
        }
        let key = b"k020";
        let mut kept: Vec<(Snapshot, Vec<u64>)> = vec![(status.snapshot(), vec![20])];
        for _ in 0..30 {
            let tx = delete(&index, &status, key, 20);
            commit(&status, tx);
            kept.push((status.snapshot(), vec![]));
            let tx = insert(&index, &status, key, 20);
            commit(&status, tx);
            kept.push((status.snapshot(), vec![20]));
        }
        for (snapshot, want) in &kept {
            assert_eq!(&ids(&index, &status, snapshot, key), want);
        }
        assert!(index.validate().expect("validate").errors.is_empty());
        // Neighbours are untouched.
        let latest = status.snapshot();
        assert_eq!(ids(&index, &status, &latest, b"k019"), vec![19]);
        assert_eq!(ids(&index, &status, &latest, b"k021"), vec![21]);
    }

    #[test]
    fn own_and_rolled_back_changes_restamp_in_place() {
        let (_dir, index) = fresh(4096);
        let status = ConcurrentTxStatus::new();
        let key = b"k";
        let tx = insert(&index, &status, key, 1);
        commit(&status, tx);
        // Out and in inside one transaction.
        let tx = status.begin();
        let snapshot = status.snapshot();
        index
            .delete_mark_tx_visible(&status, &snapshot, Some(tx), tx, key, row(1))
            .expect("delete");
        index
            .insert_tx_versioned(&status, tx, key, row(1))
            .expect("insert");
        assert_eq!(
            index
                .point_lookup_visible(&status, &snapshot, Some(tx), key)
                .expect("own view")
                .len(),
            1
        );
        status.abort(tx);
        let latest = status.snapshot();
        assert_eq!(ids(&index, &status, &latest, key), vec![1]);
        // Out, then in by a transaction that rolls back, then in again.
        let tx = delete(&index, &status, key, 1);
        commit(&status, tx);
        let tx = insert(&index, &status, key, 1);
        status.abort(tx);
        assert_eq!(
            ids(&index, &status, &status.snapshot(), key),
            Vec::<u64>::new()
        );
        let tx = insert(&index, &status, key, 1);
        commit(&status, tx);
        assert_eq!(ids(&index, &status, &status.snapshot(), key), vec![1]);
        // Only the latest version is live: the insert after the rollback
        // restamped the rolled-back cell instead of adding another.
        let cells = index.iter_all_entries().expect("entries");
        assert_eq!(cells.len(), 1, "{cells:?}");
    }
}
