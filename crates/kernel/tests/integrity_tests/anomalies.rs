//! Integrity checks that find the index and the heap disagreeing, and the
//! per-index error list.

use super::*;

#[test]
fn integrity_detects_index_minus_heap_orphan() {
    let (_temp, engine) = test_engine();
    create_kv_table_only(&engine);
    insert_kv_row(&engine, 1, "alpha");
    insert_kv_row(&engine, 2, "beta");
    create_kv_index(&engine);

    // Inject a bogus index entry pointing at a row_id the heap never saw.
    let index = lookup_kv_index(&engine);
    let phantom_row = RowId(99_999_999);
    let phantom_key = encode_logical_key_for_int(7);
    index
        .insert(
            &phantom_key,
            IndexRowRef::with_row_id(
                phantom_row,
                TuplePtr::new_with_generation(
                    PageId(1),
                    0,
                    redlinedb_kernel::format::PageGeneration::ONE,
                ),
            ),
        )
        .expect("inject phantom index entry");

    let report = engine.integrity_check_full().expect("integrity ok");
    let kv = report
        .relations
        .iter()
        .find(|r| r.relation_name == "kv")
        .expect("kv relation");
    let ix = kv
        .indexes
        .iter()
        .find(|i| i.index_name == "ix_kv_k")
        .expect("kv index");
    assert_eq!(ix.entry_count, 3);
    assert_eq!(ix.index_minus_heap, 1);
    assert_eq!(ix.heap_minus_index, 0);
    assert!(!report.is_clean(), "report should not be clean");
}

#[test]
fn integrity_detects_heap_minus_index_missing() {
    let (_temp, engine) = test_engine();
    create_kv_table_only(&engine);
    let row = insert_kv_row(&engine, 42, "answer");
    insert_kv_row(&engine, 100, "century");
    create_kv_index(&engine);

    let index = lookup_kv_index(&engine);
    let key = encode_logical_key_for_int(42);
    let entries = index
        .iter_all_entries()
        .expect("dump index")
        .into_iter()
        .filter(|entry| entry.row.row_id == row)
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 1, "expected exactly one entry for k=42");
    let target = entries.into_iter().next().unwrap();
    index
        .delete_mark(&key, target.row)
        .expect("drop index entry");

    let report = engine.integrity_check_full().expect("integrity ok");
    let kv = report
        .relations
        .iter()
        .find(|r| r.relation_name == "kv")
        .expect("kv relation");
    let ix = kv
        .indexes
        .iter()
        .find(|i| i.index_name == "ix_kv_k")
        .expect("kv index");
    assert_eq!(ix.heap_minus_index, 1);
    assert_eq!(ix.index_minus_heap, 0);
    assert!(!report.is_clean());
}

#[test]
fn integrity_check_returns_per_index_errors() {
    let (_temp, engine) = test_engine();
    create_kv_table_only(&engine);
    insert_kv_row(&engine, 1, "alpha");
    create_kv_index(&engine);

    let result = engine
        .integrity_check_per_index()
        .expect("integrity_check_per_index");
    let names: Vec<_> = result.iter().map(|(name, _)| name.as_str()).collect();
    assert!(names.contains(&"ix_kv_k"), "kv index in result: {names:?}");
    for (name, errors) in &result {
        assert!(
            errors.is_empty(),
            "expected no validation errors on {name}, got {errors:?}"
        );
    }
}
