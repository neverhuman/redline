//! S9-05: ROLLBACK TO keeps the transaction's snapshot and reservation, and
//! refuses to replay what it cannot replay faithfully.
//!
//! The kernel has no partial undo, so ROLLBACK TO rolls the whole
//! transaction back and re-executes the statements journaled before the
//! savepoint. It used to replay them in a fresh snapshot, so a row another
//! connection committed meanwhile became visible and an UPDATE could apply
//! on top of a concurrent one; it dropped a BEGIN IMMEDIATE reservation; it
//! re-evaluated random(), the clock and user functions, re-fired triggers
//! and re-ran DDL; and RELEASE of the last savepoint forgot the journal,
//! so a later ROLLBACK TO lost the statements before it.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::thread;
use std::time::Duration;

use redlinedb_sql::{BeginMode, Connection, Database, DbOptions, SqlValue, Step};

fn open() -> (tempfile::TempDir, Arc<Database>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Database::create(dir.path().join("replay.db"), DbOptions::default()).expect("db");
    (dir, db)
}

fn count(conn: &Arc<Connection>, sql: &str) -> i64 {
    let mut stmt = conn.prepare(sql).expect("prepare count");
    assert_eq!(stmt.step().expect("step"), Step::Row);
    stmt.column_i64(0).expect("count")
}

fn ints(conn: &Arc<Connection>, sql: &str) -> Vec<i64> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let mut out = Vec::new();
    while let Step::Row = stmt.step().expect("step") {
        out.push(stmt.column_i64(0).expect("int"));
    }
    out
}

static COUNTED_CALLS: AtomicI64 = AtomicI64::new(0);

/// `counted()` returns how many times it has been called.
fn dispatch(_db: usize, name: &str, _args: &[SqlValue]) -> Option<Result<SqlValue, String>> {
    (name == "counted").then(|| {
        Ok(SqlValue::Integer(
            COUNTED_CALLS.fetch_add(1, Ordering::SeqCst) + 1,
        ))
    })
}

const NOT_REPLAY_SAFE: &str = "not replay-safe";

#[test]
fn rollback_to_after_a_user_function_insert_is_rejected() {
    redlinedb_sql::udf::install_dispatch(dispatch);
    let (_dir, db) = open();
    let conn = db.connect();
    conn.execute("CREATE TABLE t(v INTEGER)").expect("create");
    conn.begin(BeginMode::Deferred).expect("begin");
    conn.execute("INSERT INTO t VALUES (counted())")
        .expect("insert");
    conn.execute("SAVEPOINT sp").expect("savepoint");
    conn.execute("INSERT INTO t VALUES (100)").expect("insert");
    let calls = COUNTED_CALLS.load(Ordering::SeqCst);
    let err = conn.execute("ROLLBACK TO sp").expect_err("rejected");
    assert!(err.to_string().contains(NOT_REPLAY_SAFE), "{err}");
    // The refusal came before any replay: the function did not run again,
    // and the transaction still holds both rows.
    assert_eq!(COUNTED_CALLS.load(Ordering::SeqCst), calls);
    assert_eq!(count(&conn, "SELECT count(*) FROM t"), 2);
    // The rows after the savepoint were not discarded, so the transaction
    // cannot commit; only a full ROLLBACK ends it.
    let err = conn.commit().expect_err("commit refused");
    assert!(err.to_string().contains("must roll back"), "{err}");
    conn.rollback().expect("rollback");
    assert_eq!(count(&conn, "SELECT count(*) FROM t"), 0);
}

#[test]
fn every_kind_of_unsafe_prefix_is_rejected_and_a_safe_one_replays() {
    let (_dir, db) = open();
    let conn = db.connect();
    conn.execute("CREATE TABLE t(v)").expect("create");
    conn.execute("CREATE TABLE audit(v)").expect("create audit");
    conn.execute("CREATE TABLE fired(v)").expect("create fired");
    conn.execute("CREATE TABLE stamped(v, at TEXT DEFAULT CURRENT_TIMESTAMP)")
        .expect("create stamped");
    conn.execute(
        "CREATE TRIGGER t_fire AFTER INSERT ON fired BEGIN INSERT INTO audit VALUES (NEW.v); END",
    )
    .expect("trigger");
    for unsafe_statement in [
        "INSERT INTO t VALUES (random())",
        "INSERT INTO t VALUES (randomblob(4))",
        "INSERT INTO t VALUES (datetime('now'))",
        "INSERT INTO t VALUES (last_insert_rowid())",
        "INSERT INTO t VALUES (changes())",
        "INSERT INTO t VALUES (1) RETURNING v",
        "UPDATE t SET v = 2 RETURNING v",
        "INSERT INTO fired VALUES (1)",
        "INSERT INTO stamped(v) VALUES (1)",
        "INSERT INTO t SELECT abs(random()) % 2",
        // A CTE is materialized when the statement is bound; what the
        // binding evaluates counts too.
        "WITH r(x) AS (SELECT random()) INSERT INTO t SELECT x FROM r",
        "INSERT INTO t SELECT x FROM (SELECT datetime('now') AS x)",
        "CREATE TABLE made_in_tx(x)",
        "CREATE INDEX t_v ON t(v)",
    ] {
        conn.begin(BeginMode::Deferred).expect("begin");
        let mut stmt = conn.prepare(unsafe_statement).expect("prepare");
        while let Step::Row = stmt.step().expect(unsafe_statement) {}
        drop(stmt);
        conn.execute("SAVEPOINT sp").expect("savepoint");
        conn.execute("INSERT INTO t VALUES (100)").expect("insert");
        let err = conn.execute("ROLLBACK TO sp").expect_err(unsafe_statement);
        assert!(
            err.to_string().contains(NOT_REPLAY_SAFE),
            "{unsafe_statement}: {err}"
        );
        conn.rollback().expect("rollback");
    }
    assert_eq!(count(&conn, "SELECT count(*) FROM t"), 0);
    assert_eq!(count(&conn, "SELECT count(*) FROM audit"), 0);

    // Unsafe statements after the savepoint are simply discarded, and an
    // outer savepoint with an empty prefix always rewinds.
    conn.begin(BeginMode::Deferred).expect("begin");
    conn.execute("INSERT INTO t VALUES (1)")
        .expect("safe prefix");
    conn.execute("SAVEPOINT a").expect("a");
    conn.execute("INSERT INTO t VALUES (random())")
        .expect("unsafe inside a");
    conn.execute("SAVEPOINT b").expect("b");
    conn.execute("INSERT INTO fired VALUES (2)").expect("fires");
    let err = conn
        .execute("ROLLBACK TO b")
        .expect_err("prefix has random()");
    assert!(err.to_string().contains(NOT_REPLAY_SAFE), "{err}");
    conn.rollback().expect("rollback");
    conn.begin(BeginMode::Deferred).expect("begin");
    conn.execute("INSERT INTO t VALUES (1)")
        .expect("safe prefix");
    conn.execute("SAVEPOINT a").expect("a");
    conn.execute("INSERT INTO t VALUES (random())")
        .expect("unsafe inside a");
    conn.execute("INSERT INTO fired VALUES (3)").expect("fires");
    conn.execute("ROLLBACK TO a")
        .expect("only the safe prefix replays");
    assert_eq!(ints(&conn, "SELECT v FROM t"), vec![1]);
    assert_eq!(count(&conn, "SELECT count(*) FROM audit"), 0);
    conn.execute("RELEASE a").expect("release");
    conn.commit().expect("commit");
    assert_eq!(ints(&conn, "SELECT v FROM t"), vec![1]);
}

#[test]
fn rollback_to_keeps_snapshot_against_concurrent_commit() {
    let (_dir, db) = open();
    let a = db.connect();
    let b = db.connect();
    a.execute("CREATE TABLE t(id INTEGER PRIMARY KEY)")
        .expect("create");
    a.begin(BeginMode::Deferred).expect("begin");
    a.execute("INSERT INTO t VALUES (1)").expect("insert");
    a.execute("SAVEPOINT sp").expect("savepoint");
    a.execute("INSERT INTO t VALUES (2)").expect("insert");
    // Another connection commits after A's transaction began.
    b.execute("INSERT INTO t VALUES (99)")
        .expect("concurrent insert");
    assert_eq!(ints(&a, "SELECT id FROM t ORDER BY id"), vec![1, 2]);
    a.execute("ROLLBACK TO sp").expect("rollback to");
    // Still A's snapshot: 99 stays invisible until A's transaction ends.
    assert_eq!(ints(&a, "SELECT id FROM t ORDER BY id"), vec![1]);
    a.execute("RELEASE sp").expect("release");
    a.commit().expect("commit");
    assert_eq!(ints(&a, "SELECT id FROM t ORDER BY id"), vec![1, 99]);
}

#[test]
fn rollback_to_in_immediate_tx_keeps_reservation() {
    let (_dir, db) = open();
    let a = db.connect();
    let b = db.connect();
    a.execute("CREATE TABLE t(id INTEGER PRIMARY KEY)")
        .expect("create");
    db.set_busy_timeout(Duration::from_millis(100));
    for mode in [BeginMode::Immediate, BeginMode::Exclusive] {
        a.begin(mode).expect("begin reserved");
        a.execute("INSERT INTO t VALUES (1)").expect("insert");
        a.execute("SAVEPOINT sp").expect("savepoint");
        a.execute("INSERT INTO t VALUES (2)").expect("insert");
        assert!(b.begin(BeginMode::Immediate).is_err(), "{mode:?} reserved");
        a.execute("ROLLBACK TO sp").expect("rollback to");
        assert!(
            b.begin(BeginMode::Immediate).is_err(),
            "{mode:?}: ROLLBACK TO gave up the reservation"
        );
        a.rollback().expect("rollback");
        b.begin(BeginMode::Immediate).expect("free after rollback");
        b.rollback().expect("rollback b");
    }
}

/// A replay that cannot re-execute leaves the transaction failed: it can
/// only be rolled back. Here the replayed UPDATE meets a row another
/// connection changed and committed after this transaction's snapshot.
#[test]
fn rollback_to_replay_failure_leaves_failed_tx() {
    let (_dir, db) = open();
    let a = db.connect();
    let b = db.connect();
    a.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v INTEGER)")
        .expect("create");
    a.execute("INSERT INTO t VALUES (1, 0)").expect("seed");
    a.begin(BeginMode::Deferred).expect("begin");
    a.execute("UPDATE t SET v = v + 1 WHERE id = 1")
        .expect("update holds the row");
    a.execute("SAVEPOINT sp").expect("savepoint");
    let writer = thread::spawn(move || {
        b.execute("UPDATE t SET v = 100 WHERE id = 1")
            .expect("waits for A, then commits");
    });
    // Let B queue on A's row lock before A rewinds.
    thread::sleep(Duration::from_millis(500));
    let err = a.execute("ROLLBACK TO sp").expect_err("replay conflicts");
    writer.join().expect("writer");
    assert!(!err.to_string().contains(NOT_REPLAY_SAFE), "{err}");
    assert!(a.in_transaction());
    let err = a.commit().expect_err("failed transaction");
    assert!(err.to_string().contains("must roll back"), "{err}");
    a.rollback().expect("rollback");
    // B's update stands; A's increment was never applied on top of it.
    assert_eq!(ints(&a, "SELECT v FROM t"), vec![100]);
}

#[test]
fn release_of_the_last_savepoint_keeps_the_journal_for_a_later_rollback_to() {
    let (_dir, db) = open();
    let conn = db.connect();
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY)")
        .expect("create");
    conn.begin(BeginMode::Deferred).expect("begin");
    conn.execute("SAVEPOINT sp").expect("savepoint");
    conn.execute("INSERT INTO t VALUES (1)").expect("insert");
    conn.execute("RELEASE sp").expect("release");
    conn.execute("SAVEPOINT again").expect("savepoint again");
    conn.execute("INSERT INTO t VALUES (10)").expect("insert");
    conn.execute("ROLLBACK TO again").expect("rollback to");
    // sqlite3 3.53.1 prints 1 and 1.
    assert_eq!(ints(&conn, "SELECT id FROM t"), vec![1]);
    conn.commit().expect("commit");
    assert_eq!(ints(&conn, "SELECT id FROM t"), vec![1]);
}
