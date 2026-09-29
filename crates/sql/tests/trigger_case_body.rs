//! A `CASE ... END` expression inside a trigger body is part of the body:
//! its `END` does not close the trigger, so the statements after it stay in
//! the trigger and run when it fires, not when the trigger is created.

use std::sync::Arc;

use redlinedb_sql::value::SqlValue;
use redlinedb_sql::{Connection, Database, DbOptions, Dialect, Step};

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let options = DbOptions {
        dialect: Some(Dialect::Sqlite),
        ..DbOptions::default()
    };
    let db = Database::create(dir.path().join("case.db"), options).expect("create db");
    (dir, db.connect())
}

fn single_int(conn: &Arc<Connection>, sql: &str) -> i64 {
    let mut stmt = conn.prepare(sql).expect("prepare");
    assert!(matches!(stmt.step().expect("step"), Step::Row), "{sql}");
    match stmt.column_value(0).expect("value") {
        SqlValue::Integer(n) => *n,
        other => panic!("{sql} answered {other:?}"),
    }
}

#[test]
fn a_case_expression_inside_a_trigger_body_keeps_the_body_whole() {
    let (_dir, conn) = open();
    conn.execute("CREATE TABLE t(a INTEGER, sign INTEGER)")
        .expect("t");
    conn.execute("CREATE TABLE audit(x INTEGER)")
        .expect("audit");
    conn.execute("INSERT INTO audit VALUES (1), (2)")
        .expect("audit rows");
    conn.execute(
        "CREATE TRIGGER tr AFTER INSERT ON t BEGIN \
           UPDATE t SET sign = CASE WHEN new.a > 0 THEN 1 ELSE -1 END WHERE rowid = new.rowid; \
           DELETE FROM audit WHERE x = 1; \
         END",
    )
    .expect("create trigger");
    assert_eq!(
        single_int(&conn, "SELECT count(*) FROM audit"),
        2,
        "created, not fired"
    );
    conn.execute("INSERT INTO t(a) VALUES (-5)").expect("fire");
    assert_eq!(single_int(&conn, "SELECT sign FROM t"), -1);
    assert_eq!(single_int(&conn, "SELECT count(*) FROM audit"), 1);
}
