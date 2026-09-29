//! Row conversion and storage-class comparison for the parallel heap scan
//! tests: SQLite values as RedlineDB values, the serial `SELECT`, a total
//! row order, and the row-set assertion.

use std::sync::Arc;

use redlinedb_sql::{Connection, SqlValue, Step};
use rusqlite::types::Value as RuValue;

use super::Rows;

pub(super) fn from_sqlite(value: RuValue) -> SqlValue {
    match value {
        RuValue::Null => SqlValue::Null,
        RuValue::Integer(v) => SqlValue::Integer(v),
        RuValue::Real(v) => SqlValue::Real(v),
        RuValue::Text(v) => SqlValue::Text(Arc::from(v)),
        RuValue::Blob(v) => SqlValue::Blob(Arc::from(v)),
    }
}

pub(super) fn serial_rows(conn: &Arc<Connection>, sql: &str) -> Rows {
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

pub(super) fn int(value: &SqlValue) -> i64 {
    match value {
        SqlValue::Integer(v) => *v,
        other => panic!("expected an INTEGER, got {other:?}"),
    }
}

pub(super) fn col_sum(rows: &Rows, col: usize) -> i64 {
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

pub(super) fn sorted(mut rows: Rows) -> Rows {
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
pub(super) fn assert_same_rows(got: &Rows, want: &Rows, what: &str) {
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
