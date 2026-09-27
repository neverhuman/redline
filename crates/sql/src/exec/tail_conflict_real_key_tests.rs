//! The unindexed INTEGER PRIMARY KEY check compares rowids only when the
//! key is a rowid. Any other key value must still find its duplicate.

use std::sync::Arc;

use tempfile::tempdir;

use redlinedb_kernel::format::RowId;

use crate::connection::{Database, DbOptions};
use crate::value::SqlValue;

use super::collect_unique_conflicts;

fn conflicts_for(key: SqlValue) -> Vec<RowId> {
    let dir = tempdir().unwrap();
    let db = Database::create(dir.path().join("real-key.db"), DbOptions::default()).unwrap();
    let conn = db.connect();
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)")
        .unwrap();
    conn.execute("INSERT INTO t(id, v) VALUES (5, 'a'), (6, 'b')")
        .unwrap();
    let table = conn
        .engine()
        .schema_snapshot()
        .tables
        .iter()
        .find(|table| table.name.as_ref() == "t")
        .cloned()
        .unwrap();
    let values = vec![key, SqlValue::Text(Arc::from("x"))];
    crate::exec::with_write_tx(&conn, |session, tx| {
        collect_unique_conflicts(&conn, session, tx, &table, &values, None)
            .map(|found| found.iter().map(|conflict| conflict.rowid).collect())
    })
    .unwrap()
}

#[test]
fn rowid_primary_check_finds_real_key_duplicate() {
    assert_eq!(conflicts_for(SqlValue::Integer(5)), vec![RowId(5)]);
    assert_eq!(conflicts_for(SqlValue::Real(5.0)), vec![RowId(5)]);
    assert_eq!(conflicts_for(SqlValue::Real(6.0)), vec![RowId(6)]);
    assert!(conflicts_for(SqlValue::Real(7.0)).is_empty());
    assert!(conflicts_for(SqlValue::Real(1e19)).is_empty());
}
