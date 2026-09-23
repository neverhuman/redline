//! int4range is half-open, point distance is Euclidean, citext ignores case.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("range.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

fn cell(value: &SqlValue) -> String {
    match value {
        SqlValue::Text(text) => text.trim_start_matches('\u{E000}').to_owned(),
        SqlValue::Integer(n) => n.to_string(),
        SqlValue::Real(n) => {
            if (n - n.round()).abs() < 1e-9 {
                format!("{}", n.round() as i64)
            } else {
                n.to_string()
            }
        }
        SqlValue::Null => "NULL".to_owned(),
        other => panic!("unexpected {other:?}"),
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
    conn.execute(sql)
        .unwrap_or_else(|err| panic!("{sql}: {err}"));
}

#[test]
fn postgres_ranges_points_and_citext() {
    unsafe { std::env::set_var("REDLINEDB_RESULT_DIALECT", "postgres") };
    let (_dir, conn) = open();
    assert_eq!(
        rows(&conn, "SELECT int4range(1,10) @> 5, int4range(1,10) @> 10"),
        "t|f"
    );
    assert_eq!(rows(&conn, "SELECT int4range(1,5) && int4range(3,9)"), "t");
    assert_eq!(rows(&conn, "SELECT point(0,0) <-> point(3,4)"), "5");
    exec(&conn, "DROP EXTENSION IF EXISTS citext");
    exec(&conn, "CREATE EXTENSION citext");
    assert_eq!(rows(&conn, "SELECT 'ABC'::citext = 'abc'::citext"), "t");
    assert_eq!(
        rows(
            &conn,
            "SELECT 'Hello'::citext = 'HELLO', 'Hello'::citext = 'hello', 'Hello'::citext <> 'world'"
        ),
        "t|t|t"
    );
    assert_eq!(
        rows(
            &conn,
            "WITH v(x) AS (VALUES ('banana'::citext),('Apple'::citext),('CHERRY'::citext)) SELECT x FROM v ORDER BY x"
        ),
        "Apple\nbanana\nCHERRY"
    );
    let err = conn
        .execute("CREATE EXTENSION vector")
        .expect_err("vector")
        .to_string();
    assert!(
        err.contains("vector") || err.contains("not supported") || err.contains("EXTENSION"),
        "{err}"
    );
}
