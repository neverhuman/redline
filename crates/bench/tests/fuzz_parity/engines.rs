//! Run one statement on each engine and turn its result into an `Outcome`:
//! rows of `Cell`s, done, or a classified error.

use super::*;

fn rusqlite_value_to_cell(value: rusqlite::types::Value) -> Cell {
    match value {
        rusqlite::types::Value::Null => Cell::Null,
        rusqlite::types::Value::Integer(v) => Cell::Integer(v),
        rusqlite::types::Value::Real(v) => Cell::Real(v),
        rusqlite::types::Value::Text(v) => Cell::Text(v),
        rusqlite::types::Value::Blob(v) => Cell::Blob(v),
    }
}

fn rldb_value_to_cell(value: redlinedb::ValueRef<'_>) -> Cell {
    match value {
        redlinedb::ValueRef::Null => Cell::Null,
        redlinedb::ValueRef::Integer(v) => Cell::Integer(v),
        redlinedb::ValueRef::Real(v) => Cell::Real(v),
        redlinedb::ValueRef::Text(v) => Cell::Text(v.to_owned()),
        redlinedb::ValueRef::Blob(v) => Cell::Blob(v.to_owned()),
    }
}

pub(super) fn run_sqlite(conn: &rusqlite::Connection, sql: &str) -> Outcome {
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            let msg = e.to_string();
            return Outcome::Error {
                class: classify(&msg),
                raw: msg,
            };
        }
    };
    let ncols = stmt.column_count();
    if ncols == 0 {
        // DDL/DML — execute and report Done.
        return match conn.execute(sql, []) {
            Ok(_) => Outcome::Done,
            Err(e) => {
                let msg = e.to_string();
                Outcome::Error {
                    class: classify(&msg),
                    raw: msg,
                }
            }
        };
    }
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut q = match stmt.query([]) {
        Ok(q) => q,
        Err(e) => {
            let msg = e.to_string();
            return Outcome::Error {
                class: classify(&msg),
                raw: msg,
            };
        }
    };
    loop {
        match q.next() {
            Ok(Some(row)) => {
                let mut current = Vec::with_capacity(ncols);
                for i in 0..ncols {
                    let value: rusqlite::types::Value = match row.get(i) {
                        Ok(v) => v,
                        Err(e) => {
                            let msg = e.to_string();
                            return Outcome::Error {
                                class: classify(&msg),
                                raw: msg,
                            };
                        }
                    };
                    current.push(rusqlite_value_to_cell(value));
                }
                rows.push(current);
            }
            Ok(None) => break,
            Err(e) => {
                let msg = e.to_string();
                return Outcome::Error {
                    class: classify(&msg),
                    raw: msg,
                };
            }
        }
    }
    Outcome::Rows(rows)
}

pub(super) fn run_redline(conn: &mut redlinedb::Connection, sql: &str) -> Outcome {
    let mut stmt = match conn.prepare_owned(sql) {
        Ok(s) => s,
        Err(e) => {
            let msg = e.to_string();
            return Outcome::Error {
                class: classify(&msg),
                raw: msg,
            };
        }
    };
    let ncols = stmt.column_count();
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    loop {
        match stmt.step() {
            Ok(redlinedb::OwnedStep::Row) => {
                let mut current = Vec::with_capacity(ncols);
                for i in 0..ncols {
                    let value = match stmt.column_ref(i) {
                        Ok(v) => v,
                        Err(e) => {
                            let msg = e.to_string();
                            return Outcome::Error {
                                class: classify(&msg),
                                raw: msg,
                            };
                        }
                    };
                    current.push(rldb_value_to_cell(value));
                }
                rows.push(current);
            }
            Ok(redlinedb::OwnedStep::Done) => break,
            Err(e) => {
                let msg = e.to_string();
                return Outcome::Error {
                    class: classify(&msg),
                    raw: msg,
                };
            }
        }
    }
    if ncols == 0 {
        // RedlineDB reports prepared DML as no-column-step-Done.
        Outcome::Done
    } else {
        Outcome::Rows(rows)
    }
}
