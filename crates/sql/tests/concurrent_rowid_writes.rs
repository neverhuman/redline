//! Two connections writing the same rowid: the second one gets an error
//! or another rowid, and never replaces the first one's row.
//!
//! An insert took no row lock and did not look at what already held the
//! rowid, so a transaction whose snapshot missed another connection's row
//! wrote its own version over it, and the other row silently disappeared.

use std::sync::Arc;
use std::time::Duration;

use redlinedb_sql::{Connection, Database, DbOptions, Step};

fn database() -> (tempfile::TempDir, Arc<Database>) {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = Database::create(dir.path().join("concurrent.db"), DbOptions::default())
        .expect("create database");
    (dir, db)
}

fn labelled(conn: &Arc<Connection>, sql: &str) -> Vec<(i64, String)> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let mut out = Vec::new();
    while stmt.step().expect("step") == Step::Row {
        out.push((
            stmt.column_i64(0).expect("id"),
            stmt.column_text(1).expect("value").to_owned(),
        ));
    }
    out
}

fn pairs(expected: &[(i64, &str)]) -> Vec<(i64, String)> {
    expected
        .iter()
        .map(|(id, value)| (*id, (*value).to_owned()))
        .collect()
}

#[test]
fn an_insert_of_a_rowid_another_transaction_holds_fails_after_it_commits() {
    let (_dir, db) = database();
    let first = db.connect();
    first
        .execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)")
        .expect("create");
    first.execute("BEGIN").expect("begin");
    first
        .execute("INSERT INTO t(id, v) VALUES (5, 'first')")
        .expect("first insert");
    let second = {
        let db = Arc::clone(&db);
        std::thread::spawn(move || {
            db.connect()
                .execute("INSERT INTO t(id, v) VALUES (5, 'second')")
                .map_err(|err| err.to_string())
        })
    };
    std::thread::sleep(Duration::from_millis(300));
    first.execute("COMMIT").expect("commit");
    let second = second.join().expect("second writer");
    let err = second.expect_err("the second insert of rowid 5 must fail");
    assert!(err.contains("UNIQUE constraint failed: t.id"), "{err}");
    assert_eq!(
        labelled(&first, "SELECT id, v FROM t"),
        pairs(&[(5, "first")])
    );
}

#[test]
fn moving_a_key_onto_a_row_committed_after_the_snapshot_fails() {
    let (_dir, db) = database();
    let mover = db.connect();
    mover
        .execute_batch(
            "CREATE TABLE a(id INTEGER PRIMARY KEY, v TEXT);
             INSERT INTO a(v) VALUES ('one'), ('two'), ('three');",
        )
        .expect("setup");
    let late = db.connect();
    late.execute("BEGIN").expect("begin");
    labelled(&late, "SELECT id, v FROM a");
    mover
        .execute("UPDATE a SET id = 0 WHERE id = 3")
        .expect("first move");
    // The row at 0 was committed after this transaction's snapshot, which
    // SQLite reports as BUSY_SNAPSHOT and RedlineDB as a serialization
    // failure; it is never written over.
    let err = late
        .execute("UPDATE a SET id = 0 WHERE id = 2")
        .expect_err("rowid 0 is taken");
    assert!(err.to_string().contains("serialization failure"), "{err}");
    late.execute("ROLLBACK").expect("rollback");
    assert_eq!(
        labelled(&mover, "SELECT id, v FROM a"),
        pairs(&[(0, "three"), (1, "one"), (2, "two")])
    );
}

#[test]
fn autoincrement_skips_a_rowid_committed_after_the_snapshot() {
    let (_dir, db) = database();
    let early = db.connect();
    early
        .execute_batch(
            "CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT, v TEXT);
             INSERT INTO t(v) VALUES ('a'), ('b'), ('c');",
        )
        .expect("setup");
    let late = db.connect();
    late.execute("BEGIN").expect("begin");
    labelled(&late, "SELECT id, v FROM t");
    early
        .execute("INSERT INTO t(v) VALUES ('early')")
        .expect("committed insert");
    late.execute("INSERT INTO t(v) VALUES ('late')")
        .expect("late insert takes another rowid");
    late.execute("COMMIT").expect("commit");
    assert_eq!(
        labelled(&early, "SELECT id, v FROM t"),
        pairs(&[(1, "a"), (2, "b"), (3, "c"), (4, "early"), (5, "late")])
    );
}

#[test]
fn concurrent_autoincrement_inserts_keep_every_committed_row() {
    const THREADS: usize = 4;
    const INSERTS: usize = 50;
    let (_dir, db) = database();
    db.connect()
        .execute("CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT, v TEXT)")
        .expect("create table");
    let committed: usize = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..THREADS)
            .map(|_| {
                let db = Arc::clone(&db);
                scope.spawn(move || {
                    let conn = db.connect();
                    (0..INSERTS)
                        .filter(|_| conn.execute("INSERT INTO t(v) VALUES ('x')").is_ok())
                        .count()
                })
            })
            .collect();
        workers.into_iter().map(|w| w.join().expect("worker")).sum()
    });
    let conn = db.connect();
    let mut stmt = conn
        .prepare("SELECT count(*), count(DISTINCT id) FROM t")
        .expect("prepare");
    assert_eq!(stmt.step().expect("step"), Step::Row);
    assert_eq!(
        (
            stmt.column_i64(0).expect("count"),
            stmt.column_i64(1).expect("distinct")
        ),
        (committed as i64, committed as i64),
        "every committed insert keeps its own row"
    );
    assert_eq!(committed, THREADS * INSERTS, "no insert should have failed");
}

#[test]
fn an_insert_or_ignore_that_waited_for_the_row_ignores_it() {
    let (_dir, db) = database();
    let first = db.connect();
    first
        .execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)")
        .expect("create");
    first.execute("BEGIN").expect("begin");
    first
        .execute("INSERT INTO t(id, v) VALUES (5, 'first')")
        .expect("first insert");
    let second = {
        let db = Arc::clone(&db);
        std::thread::spawn(move || {
            db.connect()
                .execute("INSERT OR IGNORE INTO t(id, v) VALUES (5, 'second')")
                .map_err(|err| err.to_string())
        })
    };
    std::thread::sleep(Duration::from_millis(300));
    first.execute("COMMIT").expect("commit");
    let inserted = second
        .join()
        .expect("second writer")
        .expect("INSERT OR IGNORE resolves once the first row is visible");
    assert_eq!(inserted, 0, "the conflicting row is ignored");
    assert_eq!(
        labelled(&first, "SELECT id, v FROM t"),
        pairs(&[(5, "first")])
    );
}

#[test]
fn stepping_past_a_committed_row_leaves_its_writers_free() {
    let (_dir, db) = database();
    let early = db.connect();
    early
        .execute_batch(
            "CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT, v TEXT);
             INSERT INTO t(v) VALUES ('a');",
        )
        .expect("setup");
    let late = db.connect();
    late.execute("BEGIN").expect("begin");
    labelled(&late, "SELECT id, v FROM t");
    early
        .execute("INSERT INTO t(v) VALUES ('early')")
        .expect("committed insert");
    // The late insert steps past rowid 2, which it cannot see.
    late.execute("INSERT INTO t(v) VALUES ('late')")
        .expect("late insert");
    // While `late` is still open, row 2 can be updated without waiting
    // for a row lock. The lock manager counts every acquisition that had
    // to wait, so the count says it, whatever the host's load; a wall-time
    // bound did not.
    let engine = early.engine_for_tests();
    let lock_waits = || -> u64 {
        engine
            .phase11_counters_snapshot()
            .lock_wait_us_buckets
            .iter()
            .sum()
    };
    let before = lock_waits();
    let update = {
        let db = Arc::clone(&db);
        std::thread::spawn(move || {
            db.connect()
                .execute("UPDATE t SET v = 'changed' WHERE id = 2")
                .expect("update");
        })
    };
    update.join().expect("updater");
    let waited = lock_waits() - before;
    late.execute("COMMIT").expect("commit");
    assert_eq!(
        waited, 0,
        "updating the row the insert stepped past waited for {waited} row lock(s)"
    );

    // The count does register a wait: an update of a row another open
    // transaction holds waits for its lock, here until a short timeout.
    // It runs inside BEGIN, where a timeout is not retried as in autocommit.
    let holder = db.connect();
    holder.execute("BEGIN").expect("begin");
    holder
        .execute("UPDATE t SET v = 'held' WHERE id = 1")
        .expect("hold row 1");
    let blocked = db.connect();
    blocked.set_busy_timeout(Duration::from_millis(50));
    blocked.execute("BEGIN").expect("begin");
    let before = lock_waits();
    let refused = blocked.execute("UPDATE t SET v = 'blocked' WHERE id = 1");
    assert!(refused.is_err(), "an update of a held row did not wait");
    assert!(
        lock_waits() > before,
        "a blocked update counted no lock wait"
    );
    let _ = blocked.execute("ROLLBACK");
    holder.execute("ROLLBACK").expect("rollback");
}
