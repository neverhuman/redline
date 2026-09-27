//! Strict per-statement differential lab: runs each statement against
//! bundled SQLite (rusqlite) and RedlineDB and requires the same outcome.
//!
//! Unlike `parity_oracle/harness.rs`, values are compared by storage
//! class (INTEGER 5 and REAL 5.0 are different answers) and every
//! statement's outcome is compared, not only the final query's.

#![allow(dead_code)]

use std::sync::Arc;

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use rusqlite::types::Value as RuValue;

#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Null,
    Int(i64),
    /// Compared by bit pattern so 5.0 never matches INTEGER 5.
    Real(u64),
    Text(String),
    Blob(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrKind {
    Unique,
    ForeignKey,
    NotNull,
    Datatype,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Done,
    Rows(Vec<Vec<Val>>),
    Err(ErrKind),
}

fn classify(message: &str) -> ErrKind {
    let lower = message.to_ascii_lowercase();
    if lower.contains("foreign key") {
        ErrKind::ForeignKey
    } else if lower.contains("unique") || lower.contains("primary key") {
        ErrKind::Unique
    } else if lower.contains("not null") {
        ErrKind::NotNull
    } else if lower.contains("datatype mismatch") {
        ErrKind::Datatype
    } else {
        ErrKind::Other
    }
}

fn from_sqlite(value: RuValue) -> Val {
    match value {
        RuValue::Null => Val::Null,
        RuValue::Integer(v) => Val::Int(v),
        RuValue::Real(v) => Val::Real(v.to_bits()),
        RuValue::Text(v) => Val::Text(v),
        RuValue::Blob(v) => Val::Blob(v),
    }
}

fn from_redline(value: &SqlValue) -> Val {
    match value {
        SqlValue::Null => Val::Null,
        SqlValue::Integer(v) => Val::Int(*v),
        SqlValue::Real(v) => Val::Real(v.to_bits()),
        SqlValue::Text(v) => Val::Text(v.as_ref().to_owned()),
        SqlValue::Blob(v) => Val::Blob(v.as_ref().to_vec()),
    }
}

fn run_sqlite(conn: &rusqlite::Connection, sql: &str) -> (Outcome, String) {
    let mut stmt = match conn.prepare(sql) {
        Ok(stmt) => stmt,
        Err(err) => return (Outcome::Err(classify(&err.to_string())), err.to_string()),
    };
    let width = stmt.column_count();
    if width == 0 {
        return match stmt.raw_execute() {
            Ok(_) => (Outcome::Done, String::new()),
            Err(err) => (Outcome::Err(classify(&err.to_string())), err.to_string()),
        };
    }
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
            Err(err) => return (Outcome::Err(classify(&err.to_string())), err.to_string()),
        }
    }
    (Outcome::Rows(rows), String::new())
}

fn run_redline(conn: &Arc<Connection>, sql: &str) -> (Outcome, String) {
    let mut stmt = match conn.prepare(sql) {
        Ok(stmt) => stmt,
        Err(err) => return (Outcome::Err(classify(&err.to_string())), err.to_string()),
    };
    let width = stmt.column_count();
    let mut rows = Vec::new();
    loop {
        match stmt.step() {
            Ok(Step::Row) => {
                let mut values = Vec::with_capacity(width);
                for index in 0..width {
                    values.push(from_redline(stmt.column_value(index).expect("column")));
                }
                rows.push(values);
            }
            Ok(Step::Done) => break,
            Err(err) => return (Outcome::Err(classify(&err.to_string())), err.to_string()),
        }
    }
    if width == 0 {
        (Outcome::Done, String::new())
    } else {
        (Outcome::Rows(rows), String::new())
    }
}

/// One SQLite connection and one RedlineDB database, driven in lockstep.
pub struct Lab {
    sqlite: rusqlite::Connection,
    redline: Arc<Connection>,
    _dir: tempfile::TempDir,
}

impl Lab {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let db = Database::create(dir.path().join("lab.db"), DbOptions::default())
            .expect("create redline db");
        Self {
            sqlite: rusqlite::Connection::open_in_memory().expect("open sqlite"),
            redline: db.connect(),
            _dir: dir,
        }
    }

    /// Run one statement on both engines and require the same outcome.
    pub fn step(&self, sql: &str) -> Outcome {
        let (expected, sqlite_msg) = run_sqlite(&self.sqlite, sql);
        let (actual, redline_msg) = run_redline(&self.redline, sql);
        assert_eq!(
            actual, expected,
            "divergence on `{sql}`\n  sqlite:  {expected:?} {sqlite_msg}\n  redline: {actual:?} {redline_msg}"
        );
        expected
    }

    /// Run every statement in order; each must match.
    pub fn script(&self, statements: &[&str]) {
        for sql in statements {
            self.step(sql);
        }
    }

    /// Like [`Lab::step`], and also require SQLite's outcome to be `want`
    /// so a test cannot pass by both engines agreeing on something else.
    pub fn expect(&self, sql: &str, want: Outcome) {
        let got = self.step(sql);
        assert_eq!(
            got, want,
            "sqlite outcome for `{sql}` was not the expected one"
        );
    }
}

pub fn unique_err() -> Outcome {
    Outcome::Err(ErrKind::Unique)
}

pub fn fk_err() -> Outcome {
    Outcome::Err(ErrKind::ForeignKey)
}

pub fn datatype_err() -> Outcome {
    Outcome::Err(ErrKind::Datatype)
}
