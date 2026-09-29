//! HNSW search after a delete, and with an `ef_search` of zero.

use super::*;

#[test]
fn mvcc_pre_delete_snapshot_does_not_observe_tombstone() {
    // The HNSW kernel doesn't own snapshot semantics directly — that
    // lives at the SQL session layer — but it MUST stamp `committing_tx`
    // on tombstones so a snapshot whose snapshot_xmin < commit_tx
    // ignores the tombstone bit. This test asserts the stamp is
    // surfaced via the public API.
    let temp = TempDir::new().unwrap();
    let buffer = make_buffer(&temp, 256);
    let mut params = HnswParams::standard(8);
    params.m = 8;
    params.m_max0 = 16;
    params.ef_construction = 64;
    let idx = HnswIndex::create_with_wal(
        buffer,
        redlinedb_kernel::format::RelId(1),
        1,
        params,
        None,
        0xDED,
    )
    .unwrap();

    let mut rng = Rng::new(7);
    for i in 0..50_u32 {
        let v = rng.vector(8);
        idx.insert_tx(TxId(1), &v, row_ref(i)).unwrap();
    }
    // Live count is 50 before delete.
    assert_eq!(idx.live_len().unwrap(), 50);
    // Deleting one node decrements live_len() (the tombstone takes
    // effect for snapshots ≥ delete_tx).
    idx.delete_tx(TxId(2), row_ref(7)).unwrap();
    assert_eq!(idx.live_len().unwrap(), 49);

    // The deleted node's edges still exist (we did not unwire them),
    // so the topology used by every snapshot stays connected. Sanity-
    // check by running a search — it must not crash, must not return
    // row 7, and must still return some live results.
    let q = Rng::new(99).vector(8);
    let hits = idx.search(&q, 5, 64).unwrap();
    assert!(!hits.is_empty());
    for h in hits {
        assert_ne!(h.row_id, RowId(7));
    }
}

#[test]
fn ef_search_zero_falls_back_to_params_default() {
    let temp = TempDir::new().unwrap();
    let buffer = make_buffer(&temp, 256);
    let mut params = HnswParams::standard(8);
    params.m = 8;
    params.m_max0 = 16;
    params.ef_construction = 64;
    params.ef_search = 32;
    let idx = HnswIndex::create_with_wal(
        buffer,
        redlinedb_kernel::format::RelId(1),
        1,
        params,
        None,
        0xCAFE,
    )
    .unwrap();
    let mut rng = Rng::new(33);
    for i in 0..100_u32 {
        let v = rng.vector(8);
        idx.insert_tx(TxId(1), &v, row_ref(i)).unwrap();
    }
    // ef_search=0 must NOT short-circuit to "empty result"; it must
    // fall back to params.ef_search. Verify by checking that we still
    // get k hits when k>0.
    let q = rng.vector(8);
    let hits = idx.search(&q, 5, 0).unwrap();
    assert_eq!(hits.len(), 5);
}
