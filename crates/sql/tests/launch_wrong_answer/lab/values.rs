//! Value comparison and conversion between the two engines: error classes,
//! storage-class equality, a canonical row order, and parameter binding.

use std::sync::Arc;

use redlinedb_sql::SqlValue;
use rusqlite::types::Value as RuValue;

/// Error messages that must agree between the engines. Anything else only
/// has to be an error on both sides.
const ERROR_CLASSES: &[&str] = &[
    "integer overflow",
    "datatype mismatch",
    "unique constraint",
    "not null constraint",
    "check constraint",
    "foreign key",
    "no such",
    "syntax error",
];

pub fn error_class(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    ERROR_CLASSES
        .iter()
        .find(|class| lower.contains(*class))
        .copied()
        .unwrap_or("other")
}

/// Storage-class equality: same class, identical REAL bits (NaN == NaN).
pub fn same_value(a: &SqlValue, b: &SqlValue) -> bool {
    match (a, b) {
        (SqlValue::Null, SqlValue::Null) => true,
        (SqlValue::Integer(x), SqlValue::Integer(y)) => x == y,
        (SqlValue::Real(x), SqlValue::Real(y)) => {
            x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan())
        }
        (SqlValue::Text(x), SqlValue::Text(y)) => x == y,
        (SqlValue::Blob(x), SqlValue::Blob(y)) => x == y,
        _ => false,
    }
}

pub(super) fn same_rows(a: &[Vec<SqlValue>], b: &[Vec<SqlValue>]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(ra, rb)| {
            ra.len() == rb.len() && ra.iter().zip(rb).all(|(x, y)| same_value(x, y))
        })
}

/// Total order used to compare unordered results: storage class first,
/// then the value's bits, so the sort never depends on either engine.
fn canonical_key(value: &SqlValue) -> (u8, Vec<u8>) {
    match value {
        SqlValue::Null => (0, Vec::new()),
        SqlValue::Integer(v) => (1, v.to_be_bytes().to_vec()),
        SqlValue::Real(v) => (2, v.to_bits().to_be_bytes().to_vec()),
        SqlValue::Text(v) => (3, v.as_bytes().to_vec()),
        SqlValue::Blob(v) => (4, v.to_vec()),
    }
}

pub(super) fn sort_rows(rows: &mut [Vec<SqlValue>]) {
    rows.sort_by(|a, b| {
        let ka: Vec<_> = a.iter().map(canonical_key).collect();
        let kb: Vec<_> = b.iter().map(canonical_key).collect();
        ka.cmp(&kb)
    });
}

pub(super) fn from_sqlite(value: RuValue) -> SqlValue {
    match value {
        RuValue::Null => SqlValue::Null,
        RuValue::Integer(v) => SqlValue::Integer(v),
        RuValue::Real(v) => SqlValue::Real(v),
        RuValue::Text(v) => SqlValue::Text(Arc::from(v)),
        RuValue::Blob(v) => SqlValue::Blob(Arc::from(v)),
    }
}

pub(super) fn to_sqlite(value: &SqlValue) -> RuValue {
    match value {
        SqlValue::Null => RuValue::Null,
        SqlValue::Integer(v) => RuValue::Integer(*v),
        SqlValue::Real(v) => RuValue::Real(*v),
        SqlValue::Text(v) => RuValue::Text(v.as_ref().to_owned()),
        SqlValue::Blob(v) => RuValue::Blob(v.as_ref().to_vec()),
    }
}

pub(super) fn bind_redline(
    stmt: &mut redlinedb_sql::Statement,
    params: &[SqlValue],
) -> redlinedb_sql::Result<()> {
    for (i, value) in params.iter().enumerate() {
        let slot = i + 1;
        match value {
            SqlValue::Null => stmt.bind_null(slot)?,
            SqlValue::Integer(v) => stmt.bind_i64(slot, *v)?,
            SqlValue::Real(v) => stmt.bind_f64(slot, *v)?,
            SqlValue::Text(v) => stmt.bind_text(slot, v.clone())?,
            SqlValue::Blob(v) => stmt.bind_blob(slot, v.clone())?,
        }
    }
    Ok(())
}
