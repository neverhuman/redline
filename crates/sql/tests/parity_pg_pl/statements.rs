//! Statement helpers for the plpgsql corpus tests: run a statement, render
//! its rows as text, or capture the error it raises.

use std::sync::Arc;

use redlinedb_sql::{Connection, SqlValue, Step};

fn cell(value: &SqlValue) -> String {
    match value {
        SqlValue::Text(text) => text.to_string(),
        SqlValue::Integer(n) => n.to_string(),
        SqlValue::Real(n) => n.to_string(),
        SqlValue::Null => "NULL".to_owned(),
        other => panic!("unexpected {other:?}"),
    }
}

pub(super) fn rows(conn: &Arc<Connection>, sql: &str) -> String {
    let mut stmt = conn
        .prepare(sql)
        .unwrap_or_else(|err| panic!("{sql}: {err}"));
    let mut lines = Vec::new();
    loop {
        match stmt.step().unwrap_or_else(|err| panic!("{sql}: {err}")) {
            Step::Row => {
                let mut parts = Vec::new();
                for idx in 0..stmt.column_count() {
                    parts.push(cell(stmt.column_value(idx).expect("col")));
                }
                lines.push(parts.join("|"));
            }
            Step::Done => break,
        }
    }
    lines.join("\n")
}

pub(super) fn exec(conn: &Arc<Connection>, sql: &str) {
    let mut stmt = conn
        .prepare(sql)
        .unwrap_or_else(|err| panic!("{sql}: {err}"));
    loop {
        match stmt.step().unwrap_or_else(|err| panic!("{sql}: {err}")) {
            Step::Row | Step::Done => break,
        }
    }
}

pub(super) fn script(conn: &Arc<Connection>, sql: &str) {
    exec(conn, sql);
}

/// The error preparing or stepping `sql` raises; panics if it succeeds.
pub(super) fn error_of(conn: &Arc<Connection>, sql: &str) -> String {
    let mut stmt = match conn.prepare(sql) {
        Ok(stmt) => stmt,
        Err(err) => return err.to_string(),
    };
    loop {
        match stmt.step() {
            Ok(Step::Row) => continue,
            Ok(Step::Done) => panic!("{sql} succeeded"),
            Err(err) => return err.to_string(),
        }
    }
}
