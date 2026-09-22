//! Postgres `real` is float4, so `0.1 + 0.2 = 0.3`.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("real.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

#[test]
fn real_addition_matches_float4() {
    // SAFETY: this test binary has one test, so the dialect flag stays local.
    unsafe { std::env::set_var("REDLINEDB_RESULT_DIALECT", "postgres") };
    let (_d, c) = open();
    let mut stmt = c
        .prepare(
            "SELECT 0.1::numeric + 0.2::numeric = 0.3::numeric, 0.1::real + 0.2::real = 0.3::real",
        )
        .expect("prepare");
    assert!(matches!(stmt.step().expect("step"), Step::Row));
    assert_eq!(
        stmt.column_value(0).expect("numeric").clone(),
        SqlValue::Text(Arc::from("t"))
    );
    assert_eq!(
        stmt.column_value(1).expect("real").clone(),
        SqlValue::Text(Arc::from("t"))
    );
}
