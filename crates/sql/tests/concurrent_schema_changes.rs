//! Two transactions changing the schema at once both keep their changes.
//!
//! Each schema change built the next catalog from the transaction's own
//! view and published it at commit, and the catalog lock was let go as
//! soon as the statement returned. Two transactions creating tables before
//! either committed both built on the same catalog: they gave their tables
//! the same relation and table ids, and the second commit replaced the
//! first one's catalog, so one table vanished and the other showed its
//! rows (#16). A schema change now holds a schema lock until its
//! transaction ends, so the second waits and builds on the first.

use std::sync::Arc;
use std::time::Duration;

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};

/// The first column of every row, with its type, so a row read from the
/// wrong table shows up in the assertion message.
fn values(conn: &Arc<Connection>, sql: &str) -> Vec<SqlValue> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let mut out = Vec::new();
    while stmt.step().expect("step") == Step::Row {
        out.push(stmt.column_value(0).expect("value").clone());
    }
    out
}

fn text(value: &str) -> SqlValue {
    SqlValue::Text(Arc::from(value))
}

#[test]
fn two_transactions_creating_tables_keep_both() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("schema.db");
    {
        // Long enough that the second schema change outwaits the first.
        let mut options = DbOptions {
            busy_timeout: Duration::from_secs(10),
            ..DbOptions::default()
        };
        options.engine.busy_timeout = Duration::from_secs(10);
        let db = Database::create(&path, options).expect("create database");
        let first = db.connect();
        first.execute("BEGIN").expect("begin");
        first
            .execute("CREATE TABLE z(id INTEGER PRIMARY KEY, v TEXT)")
            .expect("create z");
        first
            .execute("INSERT INTO z(v) VALUES ('z1')")
            .expect("insert z");
        let second = {
            let db = Arc::clone(&db);
            std::thread::spawn(move || -> Result<(), String> {
                let conn = db.connect();
                let run = |sql: &str| conn.execute(sql).map(|_| ()).map_err(|e| e.to_string());
                run("BEGIN")?;
                run("CREATE TABLE y(x TEXT)")?;
                run("INSERT INTO y VALUES ('y1')")?;
                run("COMMIT")
            })
        };
        std::thread::sleep(Duration::from_millis(300));
        first.execute("COMMIT").expect("commit z");
        second
            .join()
            .expect("second transaction")
            .expect("the second schema change waits, then commits");
        assert_eq!(values(&first, "SELECT v FROM z"), [text("z1")]);
        assert_eq!(values(&first, "SELECT x FROM y"), [text("y1")]);
    }
    let db = Database::open(&path, DbOptions::default()).expect("reopen");
    let conn = db.connect();
    assert_eq!(values(&conn, "SELECT v FROM z"), [text("z1")]);
    assert_eq!(values(&conn, "SELECT x FROM y"), [text("y1")]);
}
