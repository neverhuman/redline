//! `NOTIFY` and `pg_notify` are refused, not accepted as no-ops (PG-01).
//!
//! Nothing delivers a notification to a listening session, so a NOTIFY
//! that succeeded would claim a delivery that never happens. Both forms fail
//! with `unsupported capability:` until delivery exists; LISTEN bookkeeping
//! keeps working. Two sessions are in `pg_behavior_two_sessions.rs`.

use redlinedb_sql::{Connection, Database, DbOptions, Dialect, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let options = DbOptions {
        dialect: Some(Dialect::PostgresSubset),
        ..DbOptions::default()
    };
    let db = Database::create(dir.path().join("notify.db"), options).expect("db");
    (dir, db.connect())
}

fn error_of(conn: &Arc<Connection>, sql: &str) -> String {
    let mut stmt = match conn.prepare(sql) {
        Ok(stmt) => stmt,
        Err(err) => return err.to_string(),
    };
    loop {
        match stmt.step() {
            Ok(Step::Row) => continue,
            Ok(Step::Done) => panic!("{sql} succeeded"),
            Err(err) => return err.to_string(),
        }
    }
}

#[test]
fn notify_is_refused_until_delivery_exists() {
    let (_d, c) = open();
    for sql in [
        "NOTIFY beyond_no_listener",
        "NOTIFY beyond_no_listener, 'hello'",
        "NOTIFY beyond_no_listener, ''",
    ] {
        // Refused when prepared, before anything runs.
        let err = c.prepare(sql).err().map(|err| err.to_string());
        assert!(
            err.as_deref()
                .is_some_and(|err| err.contains("unsupported capability: NOTIFY")),
            "{sql}: {err:?}"
        );
    }
    for sql in [
        "SELECT pg_notify('beyond_no_listener', '') IS NULL",
        "SELECT pg_notify('beyond_no_listener', repeat('x', 100)) IS NULL",
    ] {
        let err = error_of(&c, sql);
        assert!(
            err.contains("unsupported capability: pg_notify"),
            "{sql}: {err}"
        );
    }
    // Inside a transaction the refusal leaves LISTEN bookkeeping intact.
    c.execute("BEGIN").expect("begin");
    c.execute("LISTEN beyond_ch_tx").expect("listen");
    let err = error_of(&c, "NOTIFY beyond_ch_tx, 'discarded'");
    assert!(err.contains("unsupported capability: NOTIFY"), "{err}");
    c.execute("ROLLBACK").expect("rollback");
    let mut listening = c
        .prepare("SELECT count(*) FROM (SELECT pg_listening_channels()) s")
        .expect("prepare");
    assert!(matches!(listening.step().expect("step"), Step::Row));
    assert_eq!(
        listening.column_value(0).expect("count").clone(),
        SqlValue::Integer(0)
    );
    // The connection is still usable.
    let mut one = c.prepare("SELECT 1").expect("select");
    assert!(matches!(one.step().expect("step"), Step::Row));
    assert_eq!(
        one.column_value(0).expect("one").clone(),
        SqlValue::Integer(1)
    );
}
