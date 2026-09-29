//! PG-01: the Postgres session functions do not exist in the SQLite dialect.
//!
//! They used to be resolved before any dialect check, so a default (SQLite)
//! database answered `SELECT pg_advisory_unlock(1), pg_wal_lsn_diff('0/10',
//! '0/0'), pg_notify('a','b'), txid_current()` with `1|1|1|1` where sqlite3
//! 3.53.1 says `no such function`. `repeat`, `pg_backend_pid` and the role
//! functions leaked the same way, and `NOTIFY` ran as a no-op.
//!
//! The database is opened with an explicit SQLite dialect, so the process
//! environment cannot change the answer.

use std::sync::Arc;

use redlinedb_sql::{Connection, Database, DbOptions, Dialect, Step};

fn open_sqlite() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let options = DbOptions {
        dialect: Some(Dialect::Sqlite),
        ..DbOptions::default()
    };
    let db = Database::create(dir.path().join("lite.db"), options).expect("create db");
    (dir, db.connect())
}

/// The error preparing or stepping `sql` raises; panics if it succeeds.
fn error_of(conn: &Arc<Connection>, sql: &str) -> String {
    let mut stmt = match conn.prepare(sql) {
        Ok(stmt) => stmt,
        Err(err) => return err.to_string(),
    };
    loop {
        match stmt.step() {
            Ok(Step::Row) => {
                let value = stmt.column_value(0).map(|v| v.clone());
                panic!("{sql} answered {value:?} in the SQLite dialect");
            }
            Ok(Step::Done) => panic!("{sql} succeeded in the SQLite dialect"),
            Err(err) => return err.to_string(),
        }
    }
}

#[test]
fn postgres_session_functions_are_unknown_in_the_sqlite_dialect() {
    let (_dir, conn) = open_sqlite();
    for sql in [
        "SELECT pg_backend_pid()",
        "SELECT txid_current()",
        "SELECT pg_current_xact_id()",
        "SELECT pg_current_wal_lsn()",
        "SELECT pg_wal_lsn_diff('0/10', '0/0')",
        "SELECT pg_notification_queue_usage()",
        "SELECT pg_advisory_lock(1)",
        "SELECT pg_try_advisory_lock(1)",
        "SELECT pg_advisory_unlock(1)",
        "SELECT pg_advisory_unlock_all()",
        "SELECT pg_export_snapshot()",
        "SELECT pg_notify('a', 'b')",
        "SELECT repeat('x', 3)",
        "SELECT current_user()",
        "SELECT session_user()",
        "SELECT current_role()",
        "SELECT pg_get_userbyid(10)",
        "SELECT pg_advisory_unlock(1), pg_wal_lsn_diff('0/10','0/0'), \
         pg_notify('a','b'), txid_current()",
    ] {
        let err = error_of(&conn, sql);
        assert!(
            err.contains("no such function: "),
            "{sql}: expected an unknown-function error, got {err}"
        );
    }
    // The same names inside a FROM query are unknown as well.
    conn.execute("CREATE TABLE t(x INTEGER)").expect("create");
    conn.execute("INSERT INTO t VALUES (1)").expect("insert");
    let err = error_of(&conn, "SELECT pg_try_advisory_lock(x) FROM t");
    assert!(
        err.contains("no such function: pg_try_advisory_lock"),
        "{err}"
    );
}

#[test]
fn notify_is_not_a_statement_in_the_sqlite_dialect() {
    let (_dir, conn) = open_sqlite();
    for sql in ["NOTIFY chan", "NOTIFY chan, 'payload'"] {
        let err = error_of(&conn, sql);
        assert!(!err.is_empty(), "{sql}");
    }
    // The connection is still usable afterwards.
    let mut stmt = conn.prepare("SELECT 1").expect("select");
    assert!(matches!(stmt.step().expect("step"), Step::Row));
}
