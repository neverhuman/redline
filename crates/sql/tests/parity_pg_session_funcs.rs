//! Shape checks for the thin Postgres session functions.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("sessfn.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

fn row(conn: &Arc<Connection>, sql: &str) -> Vec<SqlValue> {
    let mut stmt = conn.prepare(sql).expect(sql);
    assert!(matches!(stmt.step().expect(sql), Step::Row), "{sql}");
    (0..stmt.column_count())
        .map(|index| stmt.column_value(index).expect(sql).clone())
        .collect()
}

#[test]
fn session_functions_match_the_shell_shapes() {
    // SAFETY: this test binary has one test, so the dialect flag stays local.
    unsafe { std::env::set_var("REDLINEDB_RESULT_DIALECT", "postgres") };
    let (_d, c) = open();
    let t = SqlValue::Text(Arc::from("t"));
    let f = SqlValue::Text(Arc::from("f"));
    assert_eq!(
        row(
            &c,
            "SELECT pg_backend_pid() = pg_backend_pid(), pg_backend_pid() > 0"
        ),
        vec![t.clone(), t.clone()]
    );
    assert_eq!(row(&c, "SELECT txid_current() > 0"), vec![t.clone()]);
    assert_eq!(
        row(&c, "SELECT pg_current_xact_id()::text ~ '^[0-9]+$'"),
        vec![t.clone()]
    );
    assert_eq!(
        row(&c, "SELECT pg_advisory_lock(987001) IS NULL"),
        vec![f.clone()]
    );
    assert_eq!(
        row(&c, "SELECT pg_advisory_unlock(987001)"),
        vec![t.clone()]
    );
    assert_eq!(
        row(&c, "SELECT pg_try_advisory_lock(987002)"),
        vec![t.clone()]
    );
    assert_eq!(
        row(
            &c,
            "SELECT pg_wal_lsn_diff(pg_current_wal_lsn(), pg_current_wal_lsn())"
        ),
        vec![SqlValue::Integer(0)]
    );
    assert_eq!(
        row(&c, "SELECT length(pg_current_wal_lsn()::text) > 0"),
        vec![t.clone()]
    );
    assert_eq!(
        row(
            &c,
            "SELECT pg_notify('beyond_no_listener', 'fn-payload') IS NULL"
        ),
        vec![f]
    );
    assert_eq!(
        row(
            &c,
            "SELECT pg_notification_queue_usage() >= 0::float AND pg_notification_queue_usage() <= 1::float"
        ),
        vec![t]
    );
    assert_eq!(
        row(&c, "SELECT length(repeat('x', 100))"),
        vec![SqlValue::Integer(100)]
    );
}
