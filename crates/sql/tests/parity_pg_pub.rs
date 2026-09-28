//! Row-lock clauses, LOCK TABLE, refused snapshot export, and publications.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("pub.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

fn cell(value: &SqlValue) -> String {
    match value {
        SqlValue::Text(text) => text.to_string(),
        SqlValue::Integer(n) => n.to_string(),
        SqlValue::Null => "NULL".to_owned(),
        other => format!("{other:?}"),
    }
}

fn rows(conn: &Arc<Connection>, sql: &str) -> String {
    let mut stmt = conn
        .prepare(sql)
        .unwrap_or_else(|err| panic!("{sql}: {err}"));
    let mut lines = Vec::new();
    loop {
        match stmt.step().unwrap_or_else(|err| panic!("{sql}: {err}")) {
            Step::Row => {
                let mut parts = Vec::new();
                for idx in 0..stmt.column_count() {
                    parts.push(cell(stmt.column_value(idx).expect("col")));
                }
                lines.push(parts.join("|"));
            }
            Step::Done => break,
        }
    }
    lines.join("\n")
}

fn exec(conn: &Arc<Connection>, sql: &str) {
    let mut stmt = conn
        .prepare(sql)
        .unwrap_or_else(|err| panic!("{sql}: {err}"));
    loop {
        match stmt.step().unwrap_or_else(|err| panic!("{sql}: {err}")) {
            Step::Row | Step::Done => break,
        }
    }
}

#[test]
fn publication_lock_and_snapshot() {
    unsafe { std::env::set_var("REDLINEDB_RESULT_DIALECT", "postgres") };
    let (_dir, conn) = open();
    exec(&conn, "CREATE TABLE bm_t(id int primary key, v int)");
    exec(&conn, "INSERT INTO bm_t VALUES (1,10),(2,20)");
    exec(&conn, "BEGIN");
    assert_eq!(
        rows(&conn, "SELECT id, v FROM bm_t WHERE id = 1 FOR KEY SHARE"),
        "1|10"
    );
    exec(&conn, "COMMIT");
    assert_eq!(
        rows(
            &conn,
            "SELECT id, v FROM bm_t WHERE id = 2 FOR NO KEY UPDATE"
        ),
        "2|20"
    );
    exec(&conn, "CREATE TABLE bm_lk(id int)");
    exec(&conn, "BEGIN");
    exec(&conn, "LOCK TABLE bm_lk IN ACCESS EXCLUSIVE MODE");
    assert_eq!(rows(&conn, "SELECT count(*) FROM bm_lk"), "0");
    exec(&conn, "COMMIT");
    exec(&conn, "BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ");
    // PG-01: no session can import a snapshot, so exporting one is refused
    // instead of answered with a constant name.
    let mut export = conn
        .prepare("SELECT length(pg_export_snapshot()) > 0")
        .expect("prepare");
    let err = export.step().expect_err("pg_export_snapshot");
    assert!(
        err.to_string()
            .contains("unsupported capability: pg_export_snapshot"),
        "{err}"
    );
    drop(export);
    exec(&conn, "ROLLBACK");
    exec(&conn, "CREATE PUBLICATION beyond_pub_all FOR ALL TABLES");
    assert_eq!(
        rows(
            &conn,
            "SELECT count(*) >= 1 FROM pg_publication WHERE pubname = 'beyond_pub_all'"
        ),
        "t"
    );
    exec(&conn, "DROP PUBLICATION IF EXISTS beyond_pub_all");
    exec(&conn, "DROP PUBLICATION IF EXISTS beyond_pub_all");
}
