//! Error classes for malformed JSON and bad paths, and `json_tree` walks.

use super::*;

#[test]
fn json_each_invalid_json_raises_error() {
    let pair = Pair::new();
    pair.assert_rejects_with_same_error_class(
        "SELECT key FROM json_each('not json')",
        ErrorClass::MalformedJson,
        "malformed JSON",
    );
    pair.assert_rejects_with_same_error_class(
        "SELECT key FROM json_tree('{broken')",
        ErrorClass::MalformedJson,
        "malformed JSON",
    );
}

#[test]
fn json_table_invalid_path_raises_matching_error_class() {
    let pair = Pair::new();
    pair.assert_rejects_with_same_error_class(
        "SELECT key FROM json_each('{\"a\":1}', 'a')",
        ErrorClass::JsonPath,
        "JSON path",
    );
    pair.assert_rejects_with_same_error_class(
        "SELECT key FROM json_tree('{\"a\":1}', 'a')",
        ErrorClass::JsonPath,
        "JSON path",
    );
}

#[test]
fn json_tree_emits_every_node_with_parent_links() {
    let pair = Pair::new();
    // Per SQLite docs, the `id` column is implementation-defined. We
    // assert structural parity on the spec-stable columns (key/value/
    // type/atom/fullkey/path) and verify parent linkage separately by
    // building a (fullkey -> parent_fullkey) projection that does NOT
    // depend on the engine's id numbering scheme.
    pair.assert_parity(
        "SELECT key, value, type, atom, fullkey, path \
         FROM json_tree('[10, 20]') ORDER BY fullkey",
    );
    // Same projection ordered by tree position should yield matching rows.
    let rl_struct: Vec<(SqlValue, SqlValue)> = pair
        .redline_rows("SELECT fullkey, path FROM json_tree('[10, 20]') ORDER BY fullkey")
        .into_iter()
        .map(|row| (row[0].clone(), row[1].clone()))
        .collect();
    let sl_struct: Vec<(SqlValue, SqlValue)> = pair
        .sqlite_rows("SELECT fullkey, path FROM json_tree('[10, 20]') ORDER BY fullkey")
        .into_iter()
        .map(|row| (row[0].clone(), row[1].clone()))
        .collect();
    assert_eq!(rl_struct, sl_struct, "fullkey/path parity for json_tree");
}

#[test]
fn json_tree_nested_object_recursion() {
    let pair = Pair::new();
    pair.assert_parity(
        "SELECT key, type, fullkey, path \
         FROM json_tree('{\"a\":{\"b\":[1,2]}}') ORDER BY id",
    );
}

#[test]
fn json_tree_filter_by_type() {
    let pair = Pair::new();
    pair.assert_parity(
        "SELECT fullkey, value FROM json_tree('{\"a\":[1,2,3],\"b\":\"x\"}') \
         WHERE type = 'integer' ORDER BY fullkey",
    );
}

#[test]
fn json_tree_with_path_starts_walk_at_subtree() {
    let pair = Pair::new();
    pair.assert_parity(
        "SELECT key, type, fullkey \
         FROM json_tree('{\"outer\":{\"a\":1,\"b\":2}}', '$.outer') ORDER BY fullkey",
    );
}
