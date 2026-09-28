//! `PRAGMA redline_durability`: the commit durability the engine applies
//! right now (workplan R10).
//!
//! `PRAGMA synchronous` keeps SQLite's per-connection recall value, which
//! starts at FULL whatever mode the database was opened in, so it cannot
//! tell a caller (or the durability receipt tool) which barrier a COMMIT
//! waits for. This PRAGMA reads the engine's live mode instead.

use redlinedb_kernel::engine::{CommitDurability, EngineConfig};
use std::sync::Arc;

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use tempfile::tempdir;

fn options(durability: CommitDurability) -> DbOptions {
    DbOptions {
        engine: EngineConfig {
            commit_durability: durability,
            ..EngineConfig::default()
        },
        ..DbOptions::default()
    }
}

fn scalar(conn: &Arc<Connection>, sql: &str) -> SqlValue {
    let mut stmt = conn.prepare(sql).expect("prepare");
    assert_eq!(
        stmt.step().expect("step"),
        Step::Row,
        "{sql} returned no row"
    );
    let value = stmt.column_value(0).expect("column").clone();
    assert_eq!(
        stmt.step().expect("step"),
        Step::Done,
        "{sql} returned two rows"
    );
    value
}

fn scalar_text(conn: &Arc<Connection>, sql: &str) -> String {
    match scalar(conn, sql) {
        SqlValue::Text(text) => text.to_string(),
        other => panic!("{sql} returned {other:?}, not text"),
    }
}

#[test]
fn redline_durability_reports_the_mode_the_database_was_opened_in() {
    for (durability, expected) in [
        (CommitDurability::Strict, "strict"),
        (CommitDurability::Normal, "normal"),
        (CommitDurability::UnsafeDev, "unsafe_dev"),
    ] {
        let dir = tempdir().expect("temp dir");
        let db = Database::create(dir.path().join("db"), options(durability)).expect("create");
        let conn = db.connect();
        assert_eq!(
            scalar_text(&conn, "PRAGMA redline_durability"),
            expected,
            "opened with {durability:?}"
        );
    }
}

#[test]
fn redline_durability_follows_a_synchronous_change_that_synchronous_itself_hides() {
    let dir = tempdir().expect("temp dir");
    let db =
        Database::create(dir.path().join("db"), options(CommitDurability::Normal)).expect("create");
    let conn = db.connect();
    // The SQLite-compatible recall value says FULL although commits do not
    // wait for fsync; that is why the receipt tool cannot use it.
    assert_eq!(scalar(&conn, "PRAGMA synchronous"), SqlValue::Integer(2));
    assert_eq!(scalar_text(&conn, "PRAGMA redline_durability"), "normal");

    conn.execute("PRAGMA synchronous = FULL")
        .expect("synchronous full");
    assert_eq!(scalar_text(&conn, "PRAGMA redline_durability"), "strict");
    // Another connection on the same engine sees the same live mode.
    assert_eq!(
        scalar_text(&db.connect(), "PRAGMA redline_durability"),
        "strict"
    );

    conn.execute("PRAGMA synchronous = OFF")
        .expect("synchronous off");
    assert_eq!(scalar_text(&conn, "PRAGMA redline_durability"), "normal");
}

#[test]
fn redline_durability_is_read_only() {
    let dir = tempdir().expect("temp dir");
    let db =
        Database::create(dir.path().join("db"), options(CommitDurability::Strict)).expect("create");
    let conn = db.connect();
    let err = conn
        .execute("PRAGMA redline_durability = normal")
        .expect_err("setting the mode through this PRAGMA must fail");
    assert!(err.to_string().contains("read-only"), "{err}");
    assert_eq!(scalar_text(&conn, "PRAGMA redline_durability"), "strict");
}
