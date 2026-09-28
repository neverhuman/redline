//! S9-04: preparing a SAVEPOINT, RELEASE or ROLLBACK TO changes nothing;
//! stepping it does, every time it is stepped.
//!
//! The savepoint commands used to take effect inside `prepare_v2` and hand
//! back an already-finished statement, so preparing `RELEASE s` committed
//! the implicit transaction without a step, a prepared `ROLLBACK TO s` threw
//! work away before anyone asked, and reset + step did nothing. SQLite
//! applies them in `sqlite3_step`, like every other statement.

use std::sync::Arc;

use redlinedb_sql::{BeginMode, Connection, Database, DbOptions, Step};

fn open() -> (tempfile::TempDir, Arc<Database>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Database::create(dir.path().join("sp.db"), DbOptions::default()).expect("db");
    (dir, db)
}

fn count(conn: &Arc<Connection>, sql: &str) -> i64 {
    let mut stmt = conn.prepare(sql).expect("prepare count");
    assert_eq!(stmt.step().expect("step"), Step::Row);
    stmt.column_i64(0).expect("count")
}

#[test]
fn prepare_release_without_step_does_not_commit() {
    let (_dir, db) = open();
    let a = db.connect();
    let b = db.connect();
    a.execute("CREATE TABLE t(id INTEGER PRIMARY KEY)")
        .expect("create");
    // SAVEPOINT outside a transaction opens one; releasing it commits.
    a.execute("SAVEPOINT sp").expect("savepoint");
    a.execute("INSERT INTO t VALUES (1)").expect("insert");
    let release = a.prepare("RELEASE sp").expect("prepare release");
    assert!(release.is_readonly(), "transaction control is read-only");
    drop(release);
    assert!(a.in_transaction(), "preparing RELEASE committed");
    assert_eq!(count(&b, "SELECT count(*) FROM t"), 0);
    a.execute("ROLLBACK")
        .expect("the transaction is still open");
    assert_eq!(count(&a, "SELECT count(*) FROM t"), 0);
    assert_eq!(count(&b, "SELECT count(*) FROM t"), 0);
}

#[test]
fn prepare_rollback_to_without_step_keeps_rows() {
    let (_dir, db) = open();
    let conn = db.connect();
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY)")
        .expect("create");
    conn.begin(BeginMode::Deferred).expect("begin");
    conn.execute("INSERT INTO t VALUES (1)").expect("ins 1");
    conn.execute("SAVEPOINT sp").expect("savepoint");
    conn.execute("INSERT INTO t VALUES (2)").expect("ins 2");
    let mut rollback_to = conn.prepare("ROLLBACK TO sp").expect("prepare");
    assert_eq!(count(&conn, "SELECT count(*) FROM t"), 2);
    assert_eq!(rollback_to.step().expect("step"), Step::Done);
    assert_eq!(count(&conn, "SELECT count(*) FROM t"), 1);
    drop(rollback_to);
    conn.commit().expect("commit");
    assert_eq!(count(&conn, "SELECT count(*) FROM t"), 1);
}

#[test]
fn prepare_savepoint_without_step_opens_no_tx() {
    let (_dir, db) = open();
    let conn = db.connect();
    let mut savepoint = conn.prepare("SAVEPOINT sp").expect("prepare");
    assert!(
        !conn.in_transaction(),
        "preparing SAVEPOINT began a transaction"
    );
    // Nothing to release until the SAVEPOINT runs.
    let err = conn.execute("RELEASE sp").expect_err("no savepoint yet");
    assert!(err.to_string().contains("no such savepoint"), "{err}");
    assert_eq!(savepoint.step().expect("step"), Step::Done);
    assert!(conn.in_transaction());
    conn.execute("RELEASE sp").expect("release");
    assert!(!conn.in_transaction());
}

#[test]
fn reset_and_restep_executes_again() {
    let (_dir, db) = open();
    let conn = db.connect();
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY)")
        .expect("create");
    conn.begin(BeginMode::Deferred).expect("begin");
    let mut savepoint = conn.prepare("SAVEPOINT sp").expect("prepare");
    assert_eq!(savepoint.step().expect("first"), Step::Done);
    conn.execute("INSERT INTO t VALUES (1)").expect("insert");
    savepoint.reset().expect("reset");
    assert_eq!(savepoint.step().expect("second"), Step::Done);
    conn.execute("INSERT INTO t VALUES (2)").expect("insert");
    // Two frames named sp: the inner one rewinds only row 2.
    let mut rollback_to = conn.prepare("ROLLBACK TO sp").expect("prepare");
    assert_eq!(rollback_to.step().expect("rewind"), Step::Done);
    assert_eq!(count(&conn, "SELECT count(*) FROM t"), 1);
    conn.execute("RELEASE sp").expect("release inner");
    conn.execute("RELEASE sp").expect("release outer");
    let err = conn.execute("RELEASE sp").expect_err("both released");
    assert!(err.to_string().contains("no such savepoint"), "{err}");
    conn.commit().expect("commit");
    // A prepared ROLLBACK TO rewinds again after reset.
    conn.begin(BeginMode::Deferred).expect("begin again");
    conn.execute("SAVEPOINT again").expect("savepoint");
    let mut rewind = conn.prepare("ROLLBACK TO again").expect("prepare");
    for id in [10, 11] {
        conn.execute(&format!("INSERT INTO t VALUES ({id})"))
            .expect("insert");
        assert_eq!(rewind.step().expect("rewind"), Step::Done);
        rewind.reset().expect("reset");
        assert_eq!(count(&conn, "SELECT count(*) FROM t"), 1);
    }
    drop(rewind);
    conn.commit().expect("commit");
    assert_eq!(count(&conn, "SELECT count(*) FROM t"), 1);
}

#[test]
fn an_unknown_savepoint_is_an_error_when_stepped_not_when_prepared() {
    let (_dir, db) = open();
    let conn = db.connect();
    conn.begin(BeginMode::Deferred).expect("begin");
    for sql in [
        "RELEASE ghost",
        "ROLLBACK TO ghost",
        "ROLLBACK TRANSACTION TO SAVEPOINT ghost",
    ] {
        let mut stmt = conn.prepare(sql).expect("prepare succeeds");
        assert_eq!(stmt.sql(), sql);
        let err = stmt.step().expect_err("step fails");
        assert!(
            err.to_string().contains("no such savepoint"),
            "{sql}: {err}"
        );
    }
    conn.rollback().expect("rollback");
    // A malformed savepoint command is still a prepare-time error.
    assert!(conn.prepare("SAVEPOINT").is_err());
}
