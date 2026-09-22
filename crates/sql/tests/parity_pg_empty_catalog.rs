//! Single-node Postgres catalogs that the shell corpus only counts.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("catalog.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

fn column_text(value: &SqlValue) -> String {
    match value {
        SqlValue::Text(text) => text.to_string(),
        SqlValue::Integer(n) => n.to_string(),
        other => panic!("unexpected value {other:?}"),
    }
}

fn texts(conn: &Arc<Connection>, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let mut out = Vec::new();
    loop {
        match stmt.step().expect("step") {
            Step::Row => out.push(column_text(stmt.column_value(0).expect("value"))),
            Step::Done => break,
        }
    }
    out
}

#[test]
fn empty_catalogs_count_like_postgres() {
    // SAFETY: this test binary has one test, so the dialect flag stays local.
    unsafe { std::env::set_var("REDLINEDB_RESULT_DIALECT", "postgres") };
    let (_dir, conn) = open();
    for sql in [
        "SELECT count(*) >= 0 FROM pg_locks",
        "SELECT count(*) >= 0 FROM pg_replication_slots",
        "SELECT count(*) >= 0 FROM pg_publication",
        "SELECT count(*) >= 0 FROM pg_publication_tables",
    ] {
        assert_eq!(texts(&conn, sql), vec!["t".to_owned()], "{sql}");
    }
    for sql in [
        "SELECT count(*) FROM pg_stat_replication",
        "SELECT count(*) FROM pg_subscription",
        "SELECT count(*) FROM pg_stat_wal_receiver",
        "SELECT count(*) FROM pg_replication_slots WHERE slot_name = 'no_such_slot_for_beyond'",
    ] {
        assert_eq!(texts(&conn, sql), vec!["0".to_owned()], "{sql}");
    }
    conn.execute(
        "SELECT pg_drop_replication_slot(slot_name) FROM pg_replication_slots WHERE slot_name = 'no_such_slot_for_beyond'",
    )
    .expect("drop missing slot is not invoked");
}
