//! `CROSS JOIN LATERAL generate_series` expands once per outer row.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("lateral.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

#[test]
fn lateral_generate_series_follows_the_outer_row() {
    let (_d, c) = open();
    let mut stmt = c
        .prepare(
            "SELECT t.x, g FROM (VALUES (1),(2)) AS t(x) CROSS JOIN LATERAL generate_series(1, t.x) AS g ORDER BY t.x, g",
        )
        .expect("prepare");
    let mut rows = Vec::new();
    loop {
        match stmt.step().expect("step") {
            Step::Row => rows.push((
                stmt.column_value(0).expect("x").clone(),
                stmt.column_value(1).expect("g").clone(),
            )),
            Step::Done => break,
        }
    }
    assert_eq!(
        rows,
        vec![
            (SqlValue::Integer(1), SqlValue::Integer(1)),
            (SqlValue::Integer(2), SqlValue::Integer(1)),
            (SqlValue::Integer(2), SqlValue::Integer(2)),
        ]
    );
}
