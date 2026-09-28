//! The parallel heap scan returns the rows a `SELECT` sees: one version of
//! each row, never a superseded or deleted one.
//!
//! The parallel covering-scan gate dispatches to a heap scan run on the
//! database's Rayon pool. That scan read every tuple on every heap page, so
//! after an UPDATE it returned the old version of the row next to the new
//! one, and after a DELETE it still returned the row. These tests run the
//! dispatched scan (through `ws_c3_testing::parallel_heap_scan_select`, on
//! an installed pool, since no SQL plan reaches the dispatch today) and hold
//! its rows to the serial `SELECT` and to bundled SQLite, with strict
//! storage-class comparison: before any checkpoint, with every page still
//! only in the buffer pool; after a checkpoint; with new writes on top of a
//! checkpoint; after a reopen; inside an open transaction and after its
//! rollback; and from a snapshot older than later commits.

use std::path::PathBuf;
use std::sync::Arc;

use redlinedb_sql::ws_c3_testing::{parallel_heap_scan_select, with_current_rayon_pool};
use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use rusqlite::types::Value as RuValue;
use tempfile::TempDir;

const POOL_THREADS: usize = 4;
const ROWS: i64 = 300;

/// Plain projections with and without a WHERE. Each is compared unordered.
const QUERIES: &[&str] = &[
    "SELECT id, k, pad FROM t",
    "SELECT id, k FROM t WHERE k >= 1000",
    "SELECT id, length(pad), k % 7 FROM t WHERE id % 2 = 0",
];

type Rows = Vec<Vec<SqlValue>>;

/// Bundled SQLite and a file-backed RedlineDB database, written in lockstep.
struct Pair {
    sqlite: rusqlite::Connection,
    db: Arc<Database>,
    conn: Arc<Connection>,
    pool: Arc<rayon::ThreadPool>,
    path: PathBuf,
    _dir: TempDir,
}

impl Pair {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("scan.db");
        let db = Database::create(&path, DbOptions::default()).expect("create db");
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(POOL_THREADS)
            .build()
            .expect("rayon pool");
        Self {
            sqlite: rusqlite::Connection::open_in_memory().expect("sqlite"),
            conn: db.connect(),
            db,
            pool: Arc::new(pool),
            path,
            _dir: dir,
        }
    }

    fn exec_both(&self, sql: &str) {
        self.sqlite
            .execute_batch(sql)
            .unwrap_or_else(|err| panic!("sqlite rejected `{sql}`: {err}"));
        self.conn
            .execute(sql)
            .unwrap_or_else(|err| panic!("redline rejected `{sql}`: {err}"));
    }

    /// Insert ids in `ids` on both engines, with a 160-character pad so the
    /// table spans many heap pages.
    fn insert(&self, ids: std::ops::RangeInclusive<i64>) {
        self.exec_both("BEGIN");
        let mut lite = self
            .sqlite
            .prepare("INSERT INTO t(id, k, pad) VALUES (?1, ?2, ?3)")
            .expect("sqlite prepare");
        let mut red = self
            .conn
            .prepare("INSERT INTO t(id, k, pad) VALUES (?1, ?2, ?3)")
            .expect("redline prepare");
        for id in ids {
            let pad = format!("{id:0>160}");
            lite.execute(rusqlite::params![id, id, pad])
                .expect("sqlite insert");
            red.reset().expect("reset");
            red.clear_bindings();
            red.bind_i64(1, id).expect("bind id");
            red.bind_i64(2, id).expect("bind k");
            red.bind_text(3, Arc::from(pad.as_str())).expect("bind pad");
            while let Step::Row = red.step().expect("redline insert") {}
        }
        drop(lite);
        drop(red);
        self.exec_both("COMMIT");
    }

    /// Create the table, then update every third row, update every sixth row
    /// again, and delete every tenth, each in its own autocommit statement.
    fn seed(&self) {
        self.exec_both(
            "CREATE TABLE t(id INTEGER PRIMARY KEY, k INTEGER NOT NULL, pad TEXT NOT NULL)",
        );
        self.insert(1..=ROWS);
        self.exec_both("UPDATE t SET k = k + 1000 WHERE id % 3 = 0");
        self.exec_both("UPDATE t SET k = k + 1000, pad = pad || 'x' WHERE id % 6 = 0");
        self.exec_both("DELETE FROM t WHERE id % 10 = 7");
    }

    fn reopen(self) -> Self {
        let Pair {
            sqlite,
            db,
            conn,
            pool,
            path,
            _dir,
        } = self;
        drop(conn);
        drop(db);
        let db = Database::open(&path, DbOptions::default()).expect("reopen db");
        Self {
            sqlite,
            conn: db.connect(),
            db,
            pool,
            path,
            _dir,
        }
    }

    fn sqlite_rows(&self, sql: &str) -> Rows {
        let mut stmt = self.sqlite.prepare(sql).expect("sqlite prepare");
        let width = stmt.column_count();
        let mut rows = Vec::new();
        let mut cursor = stmt.raw_query();
        while let Some(row) = cursor.next().expect("sqlite step") {
            rows.push(
                (0..width)
                    .map(|i| from_sqlite(row.get(i).expect("sqlite value")))
                    .collect(),
            );
        }
        sorted(rows)
    }

    /// The expected rows of every query, read from SQLite now.
    fn sqlite_answers(&self) -> Vec<Rows> {
        QUERIES.iter().map(|sql| self.sqlite_rows(sql)).collect()
    }

    /// `conn` must answer every query as `want` says, through the serial
    /// `SELECT` and through the parallel heap scan: same rows, same count,
    /// same sum of `k`, no row twice.
    fn assert_reads(&self, conn: &Arc<Connection>, want: &[Rows], label: &str) {
        for (sql, want) in QUERIES.iter().zip(want) {
            let serial = serial_rows(conn, sql);
            assert_same_rows(&serial, want, &format!("{label}: serial `{sql}`"));
            let parallel = with_current_rayon_pool(Some(Arc::clone(&self.pool)), || {
                parallel_heap_scan_select(conn, sql)
            })
            .unwrap_or_else(|err| panic!("{label}: parallel `{sql}` failed: {err}"));
            let what = format!("{label}: parallel `{sql}`");
            let ids: std::collections::BTreeSet<_> =
                parallel.iter().map(|row| int(&row[0])).collect();
            assert_eq!(ids.len(), parallel.len(), "{what}: a row came back twice");
            let parallel = sorted(parallel);
            assert_same_rows(&parallel, want, &what);
            assert_eq!(parallel.len(), want.len(), "{what}: count");
            assert_eq!(col_sum(&parallel, 1), col_sum(want, 1), "{what}: sum");
        }
    }

    fn assert_matches_sqlite(&self, label: &str) {
        self.assert_reads(&self.conn, &self.sqlite_answers(), label);
    }
}

fn from_sqlite(value: RuValue) -> SqlValue {
    match value {
        RuValue::Null => SqlValue::Null,
        RuValue::Integer(v) => SqlValue::Integer(v),
        RuValue::Real(v) => SqlValue::Real(v),
        RuValue::Text(v) => SqlValue::Text(Arc::from(v)),
        RuValue::Blob(v) => SqlValue::Blob(Arc::from(v)),
    }
}

fn serial_rows(conn: &Arc<Connection>, sql: &str) -> Rows {
    let mut stmt = conn.prepare(sql).expect("redline prepare");
    let width = stmt.column_count();
    let mut rows = Vec::new();
    while let Step::Row = stmt.step().expect("redline step") {
        rows.push(
            (0..width)
                .map(|i| stmt.column_value(i).expect("column").clone())
                .collect(),
        );
    }
    sorted(rows)
}

fn int(value: &SqlValue) -> i64 {
    match value {
        SqlValue::Integer(v) => *v,
        other => panic!("expected an INTEGER, got {other:?}"),
    }
}

fn col_sum(rows: &Rows, col: usize) -> i64 {
    rows.iter().map(|row| int(&row[col])).sum()
}

/// Storage class first, then the value's bits: a total order that depends
/// on neither engine.
fn canonical_key(value: &SqlValue) -> (u8, Vec<u8>) {
    match value {
        SqlValue::Null => (0, Vec::new()),
        SqlValue::Integer(v) => (1, v.to_be_bytes().to_vec()),
        SqlValue::Real(v) => (2, v.to_bits().to_be_bytes().to_vec()),
        SqlValue::Text(v) => (3, v.as_bytes().to_vec()),
        SqlValue::Blob(v) => (4, v.to_vec()),
    }
}

fn sorted(mut rows: Rows) -> Rows {
    rows.sort_by_cached_key(|row| row.iter().map(canonical_key).collect::<Vec<_>>());
    rows
}

/// Same storage class and value, REAL compared bit for bit.
fn same_row(a: &[SqlValue], b: &[SqlValue]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| canonical_key(x) == canonical_key(y))
}

/// Fail with the first rows that differ, keyed by `id` (column 0).
fn assert_same_rows(got: &Rows, want: &Rows, what: &str) {
    if got.len() == want.len() && got.iter().zip(want).all(|(a, b)| same_row(a, b)) {
        return;
    }
    let brief = |row: &Vec<SqlValue>| -> Vec<SqlValue> {
        row.iter()
            .map(|value| match value {
                SqlValue::Text(text) if text.len() > 12 => {
                    SqlValue::Text(Arc::from(format!("…{}", &text[text.len() - 6..])))
                }
                other => other.clone(),
            })
            .collect()
    };
    let extra: Vec<_> = got
        .iter()
        .filter(|row| !want.iter().any(|w| same_row(row, w)))
        .take(4)
        .map(brief)
        .collect();
    let missing: Vec<_> = want
        .iter()
        .filter(|row| !got.iter().any(|g| same_row(row, g)))
        .take(4)
        .map(brief)
        .collect();
    panic!(
        "{what}: {} rows, want {}; unexpected {extra:?}; missing {missing:?}",
        got.len(),
        want.len()
    );
}

#[test]
fn parallel_scan_matches_serial_and_sqlite_across_checkpoint_and_reopen() {
    let pair = Pair::new();
    pair.seed();
    let engine = pair.conn.engine_for_tests();
    assert_eq!(
        engine.buffer_pool_stats().writes,
        0,
        "the table must still be only in the buffer pool"
    );
    drop(engine);
    pair.assert_matches_sqlite("before any checkpoint");

    pair.db.checkpoint().expect("checkpoint");
    pair.assert_matches_sqlite("after a checkpoint");

    // Change flushed pages again and add rows on pages past the file's end.
    pair.exec_both("UPDATE t SET k = k + 5000 WHERE id % 4 = 1");
    pair.insert(ROWS + 1..=ROWS + 60);
    pair.exec_both("DELETE FROM t WHERE id % 9 = 0");
    pair.assert_matches_sqlite("dirty pages on top of a checkpoint");

    let pair = pair.reopen();
    pair.assert_matches_sqlite("after a reopen");
    pair.db.checkpoint().expect("checkpoint");
    pair.assert_matches_sqlite("after a checkpoint of the reopened database");
}

#[test]
fn parallel_scan_reads_the_open_transactions_view_and_not_another_ones() {
    let pair = Pair::new();
    pair.seed();
    let committed = pair.sqlite_answers();
    let other = pair.db.connect();

    pair.exec_both("BEGIN");
    pair.exec_both("UPDATE t SET k = k + 7000 WHERE id % 4 = 0");
    pair.exec_both("DELETE FROM t WHERE id % 10 = 1");
    pair.exec_both("INSERT INTO t(id, k, pad) VALUES (1001, 1001, 'new'), (1002, 1002, 'new')");
    pair.assert_matches_sqlite("inside the writer's transaction");
    pair.assert_reads(&other, &committed, "another connection during the write");
    pair.db.checkpoint().expect("checkpoint");
    pair.assert_matches_sqlite("inside the writer's transaction after a checkpoint");
    pair.assert_reads(&other, &committed, "another connection after a checkpoint");

    // The rolled-back versions stay the newest tuples of their rows, so the
    // committed version under each must come from the undo chain.
    pair.exec_both("ROLLBACK");
    pair.assert_matches_sqlite("after the rollback");
    pair.assert_reads(&other, &committed, "another connection after the rollback");
}

#[test]
fn parallel_scan_reads_an_old_snapshot_through_the_undo_chain() {
    let pair = Pair::new();
    pair.seed();
    let before = pair.sqlite_answers();
    let reader = pair.db.connect();
    reader.execute("BEGIN").expect("begin");
    pair.assert_reads(&reader, &before, "old snapshot before later commits");

    // Rows already updated twice get a third version; others their first.
    pair.exec_both("UPDATE t SET k = k + 9000 WHERE id % 2 = 0");
    pair.exec_both("UPDATE t SET pad = 'third' WHERE id % 6 = 0");
    pair.exec_both("DELETE FROM t WHERE id % 10 = 3");
    pair.insert(ROWS + 1..=ROWS + 20);

    pair.assert_reads(&reader, &before, "old snapshot after later commits");
    pair.assert_matches_sqlite("new snapshot after later commits");
    pair.db.checkpoint().expect("checkpoint");
    pair.assert_reads(&reader, &before, "old snapshot after a checkpoint");
    pair.assert_matches_sqlite("new snapshot after a checkpoint");
    reader.execute("COMMIT").expect("commit");
}
