//! Strict differential lab for the launch wrong-answer regressions.
//!
//! Every statement runs on bundled SQLite (rusqlite 0.37) and on RedlineDB.
//! Values are compared by storage class: INTEGER 5 and REAL 5.0 are
//! different answers, and REAL values must match bit for bit (two NaNs
//! count as equal). An error must be matched by an error of the same class
//! (`integer overflow` against `integer overflow`, not against a wrapped
//! value, a NULL, or some other failure).

#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use rusqlite::types::Value as RuValue;

#[path = "lab/values.rs"]
mod values;

pub use values::error_class;
use values::{bind_redline, from_sqlite, same_rows, sort_rows, to_sqlite};

/// The outcome of one statement on one engine.
#[derive(Debug, Clone)]
pub enum Outcome {
    Rows(Vec<Vec<SqlValue>>),
    Err(String),
}

/// One SQLite connection and one RedlineDB database, driven in lockstep.
pub struct Lab {
    pub sqlite: rusqlite::Connection,
    pub redline: Arc<Connection>,
    /// The database `redline` is connected to, for further connections.
    pub database: Arc<Database>,
    _dir: Option<tempfile::TempDir>,
}

impl Lab {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut lab = Self::open(&dir.path().join("lab.db"));
        lab._dir = Some(dir);
        lab
    }

    /// A lab whose RedlineDB database lives at `path` (created when absent).
    pub fn open(path: &Path) -> Self {
        let db = if path.exists() {
            Database::open(path, DbOptions::default()).expect("open redline db")
        } else {
            Database::create(path, DbOptions::default()).expect("create redline db")
        };
        Self {
            sqlite: rusqlite::Connection::open_in_memory().expect("open sqlite"),
            redline: db.connect(),
            database: db,
            _dir: None,
        }
    }

    /// Run `sql` (one or more statements) on both engines; both must succeed.
    pub fn exec_both(&self, sql: &str) {
        self.sqlite
            .execute_batch(sql)
            .unwrap_or_else(|err| panic!("sqlite rejected setup `{sql}`: {err}"));
        self.redline
            .execute(sql)
            .unwrap_or_else(|err| panic!("redline rejected setup `{sql}`: {err}"));
    }

    pub fn rows_sqlite(&self, sql: &str, params: &[SqlValue]) -> Outcome {
        let mut stmt = match self.sqlite.prepare(sql) {
            Ok(stmt) => stmt,
            Err(err) => return Outcome::Err(err.to_string()),
        };
        for (i, value) in params.iter().enumerate() {
            if let Err(err) = stmt.raw_bind_parameter(i + 1, to_sqlite(value)) {
                return Outcome::Err(err.to_string());
            }
        }
        let width = stmt.column_count();
        let mut rows = Vec::new();
        let mut cursor = stmt.raw_query();
        loop {
            match cursor.next() {
                Ok(Some(row)) => {
                    let mut values = Vec::with_capacity(width);
                    for index in 0..width {
                        let value: RuValue = row.get(index).expect("sqlite column value");
                        values.push(from_sqlite(value));
                    }
                    rows.push(values);
                }
                Ok(None) => break,
                Err(err) => return Outcome::Err(err.to_string()),
            }
        }
        Outcome::Rows(rows)
    }

    pub fn rows_redline(&self, sql: &str, params: &[SqlValue]) -> Outcome {
        let mut stmt = match self.redline.prepare(sql) {
            Ok(stmt) => stmt,
            Err(err) => return Outcome::Err(err.to_string()),
        };
        if let Err(err) = bind_redline(&mut stmt, params) {
            return Outcome::Err(err.to_string());
        }
        let width = stmt.column_count();
        let mut rows = Vec::new();
        loop {
            match stmt.step() {
                Ok(Step::Row) => {
                    let mut values = Vec::with_capacity(width);
                    for index in 0..width {
                        values.push(stmt.column_value(index).expect("column").clone());
                    }
                    rows.push(values);
                }
                Ok(Step::Done) => break,
                Err(err) => return Outcome::Err(err.to_string()),
            }
        }
        Outcome::Rows(rows)
    }

    /// Require the same outcome on both engines and return SQLite's.
    pub fn assert_same(&self, sql: &str, ordered: bool) -> Outcome {
        self.assert_same_bound(sql, &[], ordered)
    }

    /// [`Lab::assert_same`] with positional parameters bound on both sides.
    pub fn assert_same_bound(&self, sql: &str, params: &[SqlValue], ordered: bool) -> Outcome {
        let expected = self.rows_sqlite(sql, params);
        let actual = self.rows_redline(sql, params);
        compare(sql, params, expected, actual, ordered)
    }

    /// [`Lab::assert_same`] and also require SQLite's rows to be `want`, so a
    /// test cannot pass by both engines agreeing on something unexpected.
    pub fn assert_rows(&self, sql: &str, want: &[Vec<SqlValue>]) {
        match self.assert_same(sql, true) {
            Outcome::Rows(rows) => assert!(
                same_rows(&rows, want),
                "sqlite answered {rows:?} for `{sql}`, the test expected {want:?}"
            ),
            Outcome::Err(err) => panic!("sqlite failed `{sql}`: {err}; expected rows {want:?}"),
        }
    }

    /// [`Lab::assert_same`] and also require SQLite to fail with `class`.
    pub fn assert_error(&self, sql: &str, class: &str) {
        self.assert_error_bound(sql, &[], class);
    }

    pub fn assert_error_bound(&self, sql: &str, params: &[SqlValue], class: &str) {
        match self.assert_same_bound(sql, params, true) {
            Outcome::Err(err) => assert_eq!(
                error_class(&err),
                class,
                "sqlite failed `{sql}` with `{err}`, expected class `{class}`"
            ),
            Outcome::Rows(rows) => panic!("sqlite answered {rows:?} for `{sql}`, expected {class}"),
        }
    }

    /// Run a mutating statement on both engines and require the same
    /// success-or-error class. Rows written are then checked by queries.
    pub fn step_both(&self, sql: &str) {
        let expected = match self.sqlite.execute_batch(sql) {
            Ok(()) => Ok(()),
            Err(err) => Err(err.to_string()),
        };
        let actual = match self.redline.execute(sql) {
            Ok(_) => Ok(()),
            Err(err) => Err(err.to_string()),
        };
        match (&expected, &actual) {
            (Ok(()), Ok(())) => {}
            (Err(e), Err(a)) if error_class(e) == error_class(a) => {}
            _ => panic!("divergence on `{sql}`\n  sqlite:  {expected:?}\n  redline: {actual:?}"),
        }
    }

    /// The query must give the same rows whether the planner is steered
    /// onto index `index` or forced to scan, and both must match SQLite.
    /// `query` holds one `{access}` placeholder in the FROM clause.
    pub fn indexed_vs_scan(&self, table: &str, index: &str, query: &str) {
        let indexed = query.replace("{access}", &format!("{table} INDEXED BY {index}"));
        let scanned = query.replace("{access}", &format!("{table} NOT INDEXED"));
        self.assert_same(&indexed, false);
        self.assert_same(&scanned, false);
    }
}

fn compare(
    sql: &str,
    params: &[SqlValue],
    expected: Outcome,
    actual: Outcome,
    ordered: bool,
) -> Outcome {
    match (&expected, &actual) {
        (Outcome::Rows(e), Outcome::Rows(a)) => {
            let (mut e_sorted, mut a_sorted) = (e.clone(), a.clone());
            if !ordered {
                sort_rows(&mut e_sorted);
                sort_rows(&mut a_sorted);
            }
            assert!(
                same_rows(&e_sorted, &a_sorted),
                "rows differ for `{sql}` params {params:?}\n  sqlite:  {e:?}\n  redline: {a:?}"
            );
        }
        (Outcome::Err(e), Outcome::Err(a)) => assert_eq!(
            error_class(e),
            error_class(a),
            "error classes differ for `{sql}` params {params:?}\n  sqlite:  {e}\n  redline: {a}"
        ),
        _ => panic!(
            "outcomes differ for `{sql}` params {params:?}\n  sqlite:  {expected:?}\n  redline: {actual:?}"
        ),
    }
    expected
}

pub fn int(v: i64) -> SqlValue {
    SqlValue::Integer(v)
}

pub fn real(v: f64) -> SqlValue {
    SqlValue::Real(v)
}

pub fn text(v: &str) -> SqlValue {
    SqlValue::Text(Arc::from(v))
}

pub fn null() -> SqlValue {
    SqlValue::Null
}
