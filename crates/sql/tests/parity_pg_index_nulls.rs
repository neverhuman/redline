//! Postgres accepts `CREATE INDEX ... NULLS FIRST`. SQLite keeps rejecting it.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("nulls.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

fn query(conn: &Arc<Connection>, sql: &str) -> Vec<SqlValue> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let mut rows = Vec::new();
    loop {
        match stmt.step().expect("step") {
            Step::Row => rows.push(stmt.column_value(0).expect("column").clone()),
            Step::Done => break,
        }
    }
    rows
}

#[test]
fn postgres_dialect_accepts_index_nulls_first() {
    // SAFETY: this test binary contains one test. Nothing else in the process
    // reads the dialect variable while it is set, and the SQLite rejection
    // tests run in a different binary.
    unsafe { std::env::set_var("REDLINEDB_RESULT_DIALECT", "postgres") };
    let (_d, c) = open();
    c.execute("CREATE TABLE bsp_nulls (x int)").expect("create");
    c.execute("INSERT INTO bsp_nulls VALUES (1),(NULL),(2)")
        .expect("insert");
    c.execute("CREATE INDEX bsp_nulls_idx ON bsp_nulls (x NULLS FIRST)")
        .expect("index");
    assert_eq!(
        query(&c, "SELECT x FROM bsp_nulls ORDER BY x NULLS FIRST"),
        vec![SqlValue::Null, SqlValue::Integer(1), SqlValue::Integer(2)]
    );
}
