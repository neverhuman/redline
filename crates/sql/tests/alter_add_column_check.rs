//! `ALTER TABLE ... ADD COLUMN` with a CHECK constraint (case 10413).
//!
//! The CHECK may name the new column and the columns before it, holds for
//! every later write, and, as in sqlite3 3.37 and later, is tested against
//! the rows already stored (with the new column at its default) so the ALTER
//! fails and changes nothing when one of them breaks it.

use std::sync::Arc;

use redlinedb_sql::{Connection, Database, DbOptions, Dialect, Step};

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let options = DbOptions {
        dialect: Some(Dialect::Sqlite),
        ..DbOptions::default()
    };
    let db = Database::create(dir.path().join("alter.db"), options).expect("create db");
    (dir, db.connect())
}

fn column_count(conn: &Arc<Connection>) -> i64 {
    let mut stmt = conn
        .prepare("SELECT count(*) FROM pragma_table_info('t')")
        .expect("prepare");
    match stmt.step().expect("step") {
        Step::Row => match stmt.column_value(0).expect("value") {
            redlinedb_sql::value::SqlValue::Integer(n) => *n,
            other => panic!("count(*) answered {other:?}"),
        },
        Step::Done => panic!("count(*) returned no row"),
    }
}

#[test]
fn an_added_check_holds_for_later_writes() {
    let (_dir, conn) = open();
    conn.execute("CREATE TABLE t(a INTEGER)").expect("create");
    conn.execute("ALTER TABLE t ADD COLUMN b INTEGER CHECK (b >= 0)")
        .expect("add column with CHECK");
    conn.execute("INSERT INTO t VALUES (1, 2)")
        .expect("a row that passes");
    let err = conn
        .execute("INSERT INTO t VALUES (1, -1)")
        .expect_err("a row that fails the CHECK");
    assert!(err.to_string().contains("CHECK constraint failed"), "{err}");
}

#[test]
fn an_added_check_may_name_earlier_columns() {
    let (_dir, conn) = open();
    conn.execute("CREATE TABLE t(a INTEGER)").expect("create");
    conn.execute("ALTER TABLE t ADD COLUMN b INTEGER CHECK (b > a)")
        .expect("add column with CHECK");
    conn.execute("INSERT INTO t VALUES (1, 2)").expect("b > a");
    conn.execute("INSERT INTO t VALUES (3, 2)")
        .expect_err("b <= a fails the CHECK");
}

#[test]
fn stored_rows_are_tested_with_the_new_default() {
    let (_dir, conn) = open();
    conn.execute("CREATE TABLE t(a INTEGER)").expect("create");
    conn.execute("INSERT INTO t VALUES (5)").expect("insert");
    let err = conn
        .execute("ALTER TABLE t ADD COLUMN b INTEGER DEFAULT 3 CHECK (b > a)")
        .expect_err("the stored row breaks the CHECK");
    assert!(err.to_string().contains("CHECK constraint failed"), "{err}");
    assert_eq!(column_count(&conn), 1, "a failed ALTER adds no column");
    // A NULL default passes a CHECK, as in SQLite.
    conn.execute("ALTER TABLE t ADD COLUMN c INTEGER CHECK (c > a)")
        .expect("NULL passes the CHECK");
    assert_eq!(column_count(&conn), 2);
}
