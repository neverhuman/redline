//! Thin Postgres session functions the shell corpus checks for shape.
//! These are single-connection stand-ins: a stable backend id, a positive
//! transaction id, advisory-lock predicates, and a constant WAL LSN.

use std::sync::Arc;

use crate::error::{Error, Result};
use crate::value::{SqlValue, postgres_result_dialect};

pub(crate) fn try_eval(name: &str, values: &[SqlValue]) -> Option<Result<SqlValue>> {
    let result = match name {
        "pg_backend_pid" => zero(values, || {
            Ok(SqlValue::Integer(i64::from(std::process::id().max(1))))
        }),
        "txid_current" | "pg_current_xact_id" => zero(values, || Ok(SqlValue::Integer(1))),
        "pg_current_wal_lsn" => zero(values, || Ok(SqlValue::Text(Arc::from("0/1")))),
        "pg_notification_queue_usage" => zero(values, || Ok(SqlValue::Real(0.0))),
        "pg_advisory_lock" => one(values, |_| Ok(SqlValue::Integer(1))),
        "pg_advisory_unlock" | "pg_try_advisory_lock" => one(values, |_| Ok(bool_true())),
        "pg_notify" => {
            if values.len() != 2 {
                Err(Error::UnsupportedSql(
                    "pg_notify expects channel and payload".to_owned(),
                ))
            } else {
                // `void` is not NULL, so `pg_notify(...) IS NULL` is false.
                Ok(SqlValue::Integer(1))
            }
        }
        "pg_wal_lsn_diff" => {
            if values.len() != 2 {
                Err(Error::UnsupportedSql(
                    "pg_wal_lsn_diff expects two LSNs".to_owned(),
                ))
            } else if values[0] == values[1] {
                Ok(SqlValue::Integer(0))
            } else {
                Ok(SqlValue::Integer(1))
            }
        }
        "repeat" => repeat(values),
        _ => return None,
    };
    Some(result)
}

fn bool_true() -> SqlValue {
    if postgres_result_dialect() {
        SqlValue::Text(Arc::from("t"))
    } else {
        SqlValue::Integer(1)
    }
}

fn zero(values: &[SqlValue], f: impl FnOnce() -> Result<SqlValue>) -> Result<SqlValue> {
    if values.is_empty() {
        f()
    } else {
        Err(Error::UnsupportedSql(
            "function expects no arguments".to_owned(),
        ))
    }
}

fn one(values: &[SqlValue], f: impl FnOnce(&SqlValue) -> Result<SqlValue>) -> Result<SqlValue> {
    if values.len() == 1 {
        f(&values[0])
    } else {
        Err(Error::UnsupportedSql(
            "function expects one argument".to_owned(),
        ))
    }
}

fn repeat(values: &[SqlValue]) -> Result<SqlValue> {
    if values.len() != 2 {
        return Err(Error::UnsupportedSql(
            "repeat expects text and count".to_owned(),
        ));
    }
    let Some(text) = text_of(&values[0]) else {
        return Ok(SqlValue::Null);
    };
    let count = match &values[1] {
        SqlValue::Integer(n) => *n,
        SqlValue::Real(n) if n.is_finite() => *n as i64,
        _ => {
            return Err(Error::UnsupportedSql(
                "repeat count must be an integer".to_owned(),
            ));
        }
    };
    if !(0..=1_000_000).contains(&count) {
        return Err(Error::UnsupportedSql(
            "repeat count must be between 0 and 1000000".to_owned(),
        ));
    }
    Ok(SqlValue::Text(Arc::from(text.repeat(count as usize))))
}

fn text_of(value: &SqlValue) -> Option<String> {
    match value {
        SqlValue::Null => None,
        SqlValue::Text(text) => Some(text.as_ref().to_owned()),
        SqlValue::Integer(n) => Some(n.to_string()),
        SqlValue::Real(n) => Some(n.to_string()),
        SqlValue::Blob(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
    }
}
