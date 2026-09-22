//! `NOTIFY` succeeds when nobody is listening.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("notify.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

#[test]
fn notify_is_a_successful_noop() {
    // SAFETY: this test binary has one test, so the dialect flag stays local.
    unsafe { std::env::set_var("REDLINEDB_RESULT_DIALECT", "postgres") };
    let (_d, c) = open();
    c.execute("NOTIFY beyond_no_listener").expect("bare");
    c.execute("NOTIFY beyond_no_listener, 'hello'")
        .expect("payload");
    c.execute("NOTIFY beyond_no_listener, ''").expect("empty");
    let mut stmt = c
        .prepare("SELECT pg_notify('beyond_no_listener', '') IS NULL")
        .expect("prepare");
    assert!(matches!(stmt.step().expect("step"), Step::Row));
    assert_eq!(
        stmt.column_value(0).expect("value").clone(),
        SqlValue::Text(Arc::from("f"))
    );
    let mut one = c.prepare("SELECT 1").expect("select");
    assert!(matches!(one.step().expect("step"), Step::Row));
    assert_eq!(
        one.column_value(0).expect("one").clone(),
        SqlValue::Integer(1)
    );
}
