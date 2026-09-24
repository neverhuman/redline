//! Official SQLite cases 93–96: fts5, highlight, rtree, and dbstat.
//!
//! SQLite result dialect stays unset. Boolean and match results are `1`/`0`.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("virtual.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

fn cell(value: &SqlValue) -> String {
    match value {
        SqlValue::Text(text) => text.to_string(),
        SqlValue::Integer(n) => n.to_string(),
        SqlValue::Real(n) => {
            let rounded = (n * 1_000_000.0).round() / 1_000_000.0;
            if (rounded - rounded.round()).abs() < 1e-9 {
                format!("{}", rounded.round() as i64)
            } else {
                format!("{rounded}")
            }
        }
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
fn sqlite_virtual_cases_93_through_96() {
    let (_dir, conn) = open();
    exec(&conn, "CREATE VIRTUAL TABLE docs USING fts5(title, body)");
    exec(
        &conn,
        "INSERT INTO docs(title,body) VALUES('one','hello world'),('two','other text')",
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT rowid,title FROM docs WHERE docs MATCH 'hello'"
        ),
        "1|one"
    );

    exec(&conn, "CREATE VIRTUAL TABLE titles USING fts5(title)");
    exec(&conn, "INSERT INTO titles(title) VALUES('hello world')");
    assert_eq!(
        rows(
            &conn,
            "SELECT highlight(titles,0,'[',']') FROM titles WHERE titles MATCH 'hello'"
        ),
        "[hello] world"
    );

    exec(
        &conn,
        "CREATE VIRTUAL TABLE boxes USING rtree(id, x1, x2, y1, y2)",
    );
    exec(
        &conn,
        "INSERT INTO boxes VALUES(1,0,10,0,10),(2,20,30,20,30)",
    );
    assert_eq!(
        rows(&conn, "SELECT id FROM boxes WHERE x1>=0 AND x2<=10"),
        "1"
    );

    exec(&conn, "CREATE TABLE t(x)");
    exec(&conn, "INSERT INTO t VALUES(1)");
    exec(&conn, "CREATE VIRTUAL TABLE temp.stat USING dbstat");
    assert_eq!(rows(&conn, "SELECT count(*)>0 FROM stat"), "1");

    let err = conn
        .prepare("CREATE VIRTUAL TABLE z USING zipfile(name)")
        .err()
        .map(|err| err.to_string())
        .unwrap_or_default();
    assert!(err.contains("CREATE VIRTUAL TABLE"), "{err}");
}

#[test]
fn sqlite_virtual_prefix_column_axis_and_dbstat() {
    let (_dir, conn) = open();
    exec(&conn, "CREATE VIRTUAL TABLE docs USING fts5(title, body)");
    exec(
        &conn,
        "INSERT INTO docs(title,body) VALUES('one','hello world'),('two','other text')",
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT title FROM docs WHERE docs MATCH 'hel*' ORDER BY title"
        ),
        "one"
    );
    assert_eq!(
        rows(&conn, "SELECT title FROM docs WHERE body MATCH 'world'"),
        "one"
    );
    exec(&conn, "CREATE VIRTUAL TABLE axis USING rtree(id, x1, x2)");
    exec(&conn, "INSERT INTO axis VALUES(1,0,10),(2,20,30)");
    assert_eq!(
        rows(&conn, "SELECT id FROM axis WHERE x1>=0 AND x1<=5"),
        "1"
    );
    exec(&conn, "CREATE TABLE a(x)");
    exec(&conn, "INSERT INTO a VALUES(1)");
    exec(&conn, "CREATE TABLE b(y)");
    exec(&conn, "INSERT INTO b VALUES(2)");
    exec(&conn, "CREATE VIRTUAL TABLE temp.stat USING dbstat");
    assert_eq!(rows(&conn, "SELECT count(*)>0 FROM stat"), "1");
}
