//! A statement that does not spill touches no directory.
//!
//! Every statement built a query-memory broker, and the broker created its
//! spill directory up front: a failing `mkdir` (and a `statx`) per SELECT,
//! even a point read that never spills. The directory is now made only when
//! a query actually spills, as the spill files themselves already were.

use std::path::Path;

use redlinedb_sql::{Database, DbOptions, Step};

fn run(conn: &std::sync::Arc<redlinedb_sql::Connection>, sql: &str) {
    let mut stmt = conn.prepare(sql).expect("prepare");
    while stmt.step().expect("step") == Step::Row {}
}

fn open_with_spill_root(
    dir: &Path,
    spill_root: &Path,
) -> std::sync::Arc<redlinedb_sql::Connection> {
    let options = DbOptions {
        temp_dir: Some(spill_root.to_path_buf()),
        ..DbOptions::default()
    };
    Database::create(dir.join("spill-root.db"), options)
        .expect("create database")
        .connect()
}

#[test]
fn a_statement_that_does_not_spill_creates_no_directory() {
    let dir = tempfile::tempdir().expect("temp dir");
    let spill_root = dir.path().join("not-yet");
    let conn = open_with_spill_root(dir.path(), &spill_root);
    // Other code may create the root when the database opens; the check is
    // that statements do not make it again, so start from its absence.
    let _ = std::fs::remove_dir_all(&spill_root);
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)")
        .expect("create");
    conn.execute("INSERT INTO t(v) VALUES ('a'), ('b')")
        .expect("insert");
    for _ in 0..10 {
        run(&conn, "SELECT v FROM t WHERE id = 1");
        run(&conn, "SELECT count(*) FROM t");
    }
    assert!(
        !spill_root.exists(),
        "a statement that never spilled created {}",
        spill_root.display()
    );
}
