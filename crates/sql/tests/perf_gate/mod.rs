//! Work-growth gates for integration tests.
//!
//! A gate runs one operation against a table of [`SMALL`] rows and against
//! one of [`LARGE`] rows, counts the work it does with the kernel's
//! `observe` counters, and fails when that work grows with the table. The
//! counters are exact, so a gate is deterministic where a timing test is not.
//!
//! The counters are process-wide. Under nextest each test is its own
//! process; under `cargo test` the tests of one binary share a process, so
//! every gate measures while holding [`exclusive`], and a binary that uses
//! these gates should hold only gated tests.

#![allow(dead_code)]

use std::sync::{Arc, Mutex, MutexGuard};

use redlinedb_kernel::observe::{self, ObserveSnapshot};
use redlinedb_sql::{Connection, Database, DbOptions, Step};

/// Rows in the small table. Loading [`LARGE`] rows must stay quick on a
/// parent whose writes scan the table, so the sizes are modest.
pub const SMALL: i64 = 250;
/// Rows in the large table: eight times [`SMALL`].
pub const LARGE: i64 = 2_000;

static COUNTING: Mutex<()> = Mutex::new(());

/// Held while a gate measures, so no other test in this process adds work.
pub fn exclusive() -> MutexGuard<'static, ()> {
    COUNTING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The counters' change while `op` runs.
pub fn work<T>(op: impl FnOnce() -> T) -> (T, ObserveSnapshot) {
    let before = observe::snapshot();
    let out = op();
    (out, observe::snapshot().since(before))
}

/// Assert that the work `measure(rows)` counts for one operation on a table
/// of `rows` rows stays flat: at [`LARGE`] rows it is at most `cap`, and at
/// most `slack` more than at [`SMALL`] rows.
pub fn assert_flat(what: &str, cap: u64, slack: u64, mut measure: impl FnMut(i64) -> u64) {
    let _guard = exclusive();
    let small = measure(SMALL);
    let large = measure(LARGE);
    assert!(
        large <= cap && large <= small + slack,
        "{what}: {small} at {SMALL} rows but {large} at {LARGE} rows (cap {cap}, slack {slack})"
    );
}

/// A fresh database in a temporary directory.
pub fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = Database::create(dir.path().join("perf-gate.db"), DbOptions::default())
        .expect("create database");
    let conn = db.connect();
    (dir, conn)
}

/// Fill `table(id, v)` with ids `1..=rows` in one transaction.
pub fn fill(conn: &Arc<Connection>, insert_sql: &str, rows: i64) {
    conn.execute("BEGIN").expect("begin");
    let mut stmt = conn.prepare(insert_sql).expect("prepare insert");
    for id in 1..=rows {
        stmt.reset().expect("reset");
        stmt.bind_i64(1, id).expect("bind id");
        stmt.bind_text(2, "value").expect("bind value");
        while stmt.step().expect("insert") == Step::Row {}
    }
    drop(stmt);
    conn.execute("COMMIT").expect("commit");
}

/// Run `sql` with integer parameters, discarding any rows.
pub fn run(conn: &Arc<Connection>, sql: &str, params: &[i64]) {
    let mut stmt = conn.prepare(sql).expect("prepare");
    for (index, value) in params.iter().enumerate() {
        stmt.bind_i64(index + 1, *value).expect("bind");
    }
    while stmt.step().expect("step") == Step::Row {}
}
