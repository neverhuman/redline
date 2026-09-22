//! Session SHOW values the Postgres shell corpus compares.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("session.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

fn q1(conn: &Arc<Connection>, sql: &str) -> SqlValue {
    let mut stmt = conn.prepare(sql).expect("prepare");
    assert!(matches!(stmt.step().expect("step"), Step::Row));
    stmt.column_value(0).expect("value").clone()
}

#[test]
fn money_renders_locale_c() {
    let (_d, c) = open();
    assert_eq!(
        q1(&c, "SELECT 123.45::money"),
        SqlValue::Text(Arc::from("$123.45"))
    );
    assert_eq!(
        q1(&c, "SELECT (-123.45)::money"),
        SqlValue::Text(Arc::from("-$123.45"))
    );
    assert_eq!(
        q1(&c, "SELECT 1234.5::money"),
        SqlValue::Text(Arc::from("$1,234.50"))
    );
}

#[test]
fn show_wal_level_and_replication_role() {
    let (_d, c) = open();
    assert_eq!(
        q1(&c, "SHOW wal_level"),
        SqlValue::Text(Arc::from("replica"))
    );
    assert_eq!(
        q1(&c, "SHOW session_replication_role"),
        SqlValue::Text(Arc::from("origin"))
    );
}

#[test]
fn search_path_show_and_current_schema() {
    let (_d, c) = open();
    c.execute("SET search_path TO sch_sp, public").expect("set");
    assert_eq!(
        q1(&c, "SHOW search_path"),
        SqlValue::Text(Arc::from("sch_sp, public"))
    );
    assert_eq!(
        q1(&c, "SELECT current_schema()"),
        SqlValue::Text(Arc::from("sch_sp"))
    );
    c.execute("SET search_path TO ''").expect("empty");
    assert_eq!(
        q1(&c, "SHOW search_path"),
        SqlValue::Text(Arc::from("\"\""))
    );
}

#[test]
fn timestamp_subtraction_is_a_day_interval() {
    let (_d, c) = open();
    assert_eq!(
        q1(
            &c,
            "SELECT '2025-01-02'::timestamp - '2025-01-01'::timestamp"
        ),
        SqlValue::Text(Arc::from("1 day"))
    );
    assert_eq!(
        q1(&c, "SELECT '2025-01-01'::date - '2025-01-03'::date"),
        SqlValue::Text(Arc::from("-2 days"))
    );
}
