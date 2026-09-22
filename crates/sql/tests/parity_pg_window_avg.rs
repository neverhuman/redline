//! Postgres window `avg` of integers is numeric with 16 fractional digits.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("avg.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

fn query(conn: &Arc<Connection>, sql: &str) -> Vec<Vec<SqlValue>> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let width = stmt.column_count();
    let mut rows = Vec::new();
    loop {
        match stmt.step().expect("step") {
            Step::Row => {
                let mut row = Vec::with_capacity(width);
                for index in 0..width {
                    row.push(stmt.column_value(index).expect("column").clone());
                }
                rows.push(row);
            }
            Step::Done => break,
        }
    }
    rows
}

#[test]
fn window_avg_of_integers_has_numeric_scale() {
    // SAFETY: this test binary has one test, so the dialect flag stays local.
    unsafe { std::env::set_var("REDLINEDB_RESULT_DIALECT", "postgres") };
    let (_d, c) = open();
    let rows = query(
        &c,
        "WITH v(g,x) AS (VALUES (1,10),(1,20),(2,5)) SELECT g, x, sum(x) OVER w, avg(x) OVER w FROM v WINDOW w AS (PARTITION BY g) ORDER BY g, x",
    );
    let num = |text: &str| SqlValue::Text(Arc::from(text));
    assert_eq!(
        rows,
        vec![
            vec![
                SqlValue::Integer(1),
                SqlValue::Integer(10),
                SqlValue::Integer(30),
                num("15.0000000000000000"),
            ],
            vec![
                SqlValue::Integer(1),
                SqlValue::Integer(20),
                SqlValue::Integer(30),
                num("15.0000000000000000"),
            ],
            vec![
                SqlValue::Integer(2),
                SqlValue::Integer(5),
                SqlValue::Integer(5),
                num("5.0000000000000000"),
            ],
        ]
    );
}
