//! Enum order follows the declaration, and a domain check rejects a bad value.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("enum.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

fn cell(value: &SqlValue) -> String {
    match value {
        SqlValue::Text(text) => text.to_string(),
        SqlValue::Integer(n) => n.to_string(),
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
fn postgres_enum_order_and_domain_check() {
    // SAFETY: this test binary has one test, so the dialect flag stays local.
    unsafe { std::env::set_var("REDLINEDB_RESULT_DIALECT", "postgres") };
    let (_dir, conn) = open();

    exec(&conn, "DROP TYPE IF EXISTS mood CASCADE");
    exec(&conn, "CREATE TYPE mood AS ENUM ('happy','sad','meh')");
    assert_eq!(
        rows(&conn, "SELECT 'happy'::mood, 'sad'::mood < 'happy'::mood"),
        "happy|f"
    );
    // Alphabetical order would say meh < sad. Declaration order says sad comes first.
    assert_eq!(rows(&conn, "SELECT 'meh'::mood < 'sad'::mood"), "f");

    exec(&conn, "DROP TYPE IF EXISTS color CASCADE");
    exec(&conn, "CREATE TYPE color AS ENUM ('red','green','blue')");
    let err = conn
        .prepare("SELECT 'purple'::color")
        .err()
        .map(|err| err.to_string())
        .or_else(|| {
            let mut stmt = conn.prepare("SELECT 'purple'::color").ok()?;
            match stmt.step() {
                Err(err) => Some(err.to_string()),
                Ok(_) => None,
            }
        })
        .expect("bad enum value");
    assert!(err.contains("invalid input value for enum color"), "{err}");

    exec(&conn, "DROP DOMAIN IF EXISTS positive_int CASCADE");
    exec(
        &conn,
        "CREATE DOMAIN positive_int AS INTEGER CHECK (VALUE > 0)",
    );
    assert_eq!(rows(&conn, "SELECT 5::positive_int"), "5");

    exec(&conn, "DROP DOMAIN IF EXISTS positive_int2 CASCADE");
    exec(
        &conn,
        "CREATE DOMAIN positive_int2 AS INTEGER CHECK (VALUE > 0)",
    );
    let err = conn.execute("SELECT (-5)::positive_int2");
    let message = match err {
        Err(err) => err.to_string(),
        Ok(_) => {
            let mut stmt = conn.prepare("SELECT (-5)::positive_int2").expect("prepare");
            match stmt.step() {
                Err(err) => err.to_string(),
                Ok(_) => panic!("negative domain value was accepted"),
            }
        }
    };
    assert!(
        message.contains("violates check constraint \"positive_int2_check\""),
        "{message}"
    );
}
