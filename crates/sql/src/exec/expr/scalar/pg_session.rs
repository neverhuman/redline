//! Postgres session functions (PG-01).
//!
//! They exist only in the Postgres dialect: a SQLite-dialect connection gets
//! the ordinary unknown-function error, as sqlite3 does. Each one either
//! does the work its name promises or refuses with
//! [`Error::UnsupportedCapability`]; none answers with a success-shaped
//! constant.
//!
//! - Real: `txid_current` / `pg_current_xact_id` (the open transaction's
//!   id), `pg_wal_lsn_diff` (arithmetic on two LSNs), the session-level
//!   advisory locks (see `crate::pg_advisory`).
//! - Partial: `pg_backend_pid` is the OS process id, shared by every
//!   connection; `current_user` and friends name the single implicit role;
//!   `pg_notification_queue_usage` is 0 because nothing can be queued.
//! - Refused: `pg_current_wal_lsn` (the WAL position is not exposed to SQL),
//!   `pg_export_snapshot` (no session can import a snapshot) and `pg_notify`
//!   (nothing delivers a notification).

use std::cell::Cell;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::pg_advisory::AdvisoryKey;
use crate::value::{SqlValue, postgres_bool, postgres_result_dialect};

pub(crate) fn try_eval(name: &str, values: &[SqlValue]) -> Option<Result<SqlValue>> {
    if !postgres_result_dialect() {
        return None;
    }
    let result = match name {
        "pg_backend_pid" => zero(values, || {
            Ok(SqlValue::Integer(i64::from(std::process::id().max(1))))
        }),
        "txid_current" => zero(values, || current_transaction_id("txid_current")),
        "pg_current_xact_id" => zero(values, || current_transaction_id("pg_current_xact_id")),
        "pg_current_wal_lsn" => Err(Error::UnsupportedCapability {
            feature: "pg_current_wal_lsn",
            detail: "the WAL position is not exposed to SQL".to_owned(),
        }),
        "pg_export_snapshot" => Err(Error::UnsupportedCapability {
            feature: "pg_export_snapshot",
            detail: "no other session can import a snapshot".to_owned(),
        }),
        "pg_notify" => Err(Error::UnsupportedCapability {
            feature: "pg_notify",
            detail: "notifications are not delivered to listening sessions".to_owned(),
        }),
        "pg_notification_queue_usage" => zero(values, || Ok(SqlValue::Real(0.0))),
        "pg_wal_lsn_diff" => lsn_diff(values),
        "pg_advisory_lock" => advisory("pg_advisory_lock", values, |locks, key, owner| {
            locks.lock(key, owner)?;
            Ok(void())
        }),
        "pg_try_advisory_lock" => advisory("pg_try_advisory_lock", values, |locks, key, owner| {
            Ok(postgres_bool(locks.try_lock(key, owner)))
        }),
        "pg_advisory_unlock" => advisory("pg_advisory_unlock", values, |locks, key, owner| {
            Ok(postgres_bool(locks.unlock(key, owner)))
        }),
        "pg_advisory_unlock_all" => zero(values, || {
            not_while_preparing("pg_advisory_unlock_all")?;
            let conn = session_connection("pg_advisory_unlock_all")?;
            let (locks, owner) = conn.advisory_locks();
            locks.unlock_all(owner);
            Ok(void())
        }),
        "repeat" => repeat(values),
        "current_user" | "session_user" | "current_role" => zero(values, || {
            Ok(SqlValue::Text(Arc::from(crate::pg_schema::SESSION_ROLE)))
        }),
        "pg_get_userbyid" => one(values, |value| match value {
            SqlValue::Integer(10) => Ok(SqlValue::Text(Arc::from(crate::pg_schema::SESSION_ROLE))),
            SqlValue::Text(text) if text.as_ref() == "10" => {
                Ok(SqlValue::Text(Arc::from(crate::pg_schema::SESSION_ROLE)))
            }
            _ => Ok(SqlValue::Null),
        }),
        _ => return None,
    };
    Some(result)
}

thread_local! {
    /// How many statement preparations are running on this thread.
    static PREPARING: Cell<u32> = const { Cell::new(0) };
}

/// Marks a statement preparation on this thread. VALUES lists, CTEs,
/// derived tables and views can be evaluated while a statement is prepared,
/// and a cached template is not prepared again; a function that takes a
/// lock or reads the transaction would then act once, at the wrong time.
pub(crate) struct PrepareScope;

impl PrepareScope {
    pub(crate) fn enter() -> Self {
        PREPARING.with(|depth| depth.set(depth.get() + 1));
        Self
    }
}

impl Drop for PrepareScope {
    fn drop(&mut self) {
        PREPARING.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Refuses `function` while a statement is being prepared.
fn not_while_preparing(function: &'static str) -> Result<()> {
    if PREPARING.with(Cell::get) == 0 {
        return Ok(());
    }
    Err(Error::UnsupportedCapability {
        feature: function,
        detail: "it would run once, when the statement is prepared: VALUES lists, CTEs, \
                 derived tables and views are evaluated then; call it from a SELECT list \
                 or a WHERE clause"
            .to_owned(),
    })
}

/// Whether evaluating a call to `name` reads the statement's transaction,
/// so a FROM-less SELECT that calls it must run inside one.
pub(crate) fn reads_transaction(name: &str) -> bool {
    name.eq_ignore_ascii_case("txid_current") || name.eq_ignore_ascii_case("pg_current_xact_id")
}

/// psql prints a `void` result as an empty cell; it is not NULL.
fn void() -> SqlValue {
    SqlValue::Text(Arc::from(""))
}

fn current_transaction_id(function: &'static str) -> Result<SqlValue> {
    not_while_preparing(function)?;
    let Some(tx) = crate::exec::current_tx() else {
        return Err(Error::TransactionState("txid_current needs a transaction"));
    };
    // SAFETY: the executor installs the pointer for the synchronous
    // evaluation of the current statement and clears it afterwards.
    let id = unsafe { &*tx }.id().0;
    i64::try_from(id)
        .map(SqlValue::Integer)
        .map_err(|_| Error::UnsupportedCapability {
            feature: function,
            detail: format!("transaction id {id} does not fit a bigint"),
        })
}

fn session_connection(function: &'static str) -> Result<&'static crate::Connection> {
    crate::exec::current_connection().ok_or(Error::UnsupportedCapability {
        feature: function,
        detail: "advisory locks need a connection".to_owned(),
    })
}

/// The strict one-`bigint` or two-`int` advisory lock functions.
fn advisory(
    function: &'static str,
    values: &[SqlValue],
    f: impl FnOnce(&crate::pg_advisory::AdvisoryLocks, AdvisoryKey, u64) -> Result<SqlValue>,
) -> Result<SqlValue> {
    not_while_preparing(function)?;
    if values.iter().any(|value| matches!(value, SqlValue::Null)) {
        return Ok(SqlValue::Null);
    }
    let key = match values {
        [key] => AdvisoryKey::One(lock_key(key)?),
        [high, low] => AdvisoryKey::Two(int_key(high)?, int_key(low)?),
        _ => {
            return Err(Error::UnsupportedSql(
                "advisory lock functions take one bigint key or two integer keys".to_owned(),
            ));
        }
    };
    let conn = session_connection(function)?;
    let (locks, owner) = conn.advisory_locks();
    f(locks, key, owner)
}

fn lock_key(value: &SqlValue) -> Result<i64> {
    match value {
        SqlValue::Integer(n) => Ok(*n),
        SqlValue::Text(text) => text.trim().parse::<i64>().map_err(|_| {
            Error::UnsupportedSql(format!("invalid input syntax for type bigint: \"{text}\""))
        }),
        _ => Err(Error::UnsupportedSql(
            "advisory lock keys are bigint or integer values".to_owned(),
        )),
    }
}

fn int_key(value: &SqlValue) -> Result<i32> {
    i32::try_from(lock_key(value)?)
        .map_err(|_| Error::UnsupportedSql("integer out of range".to_owned()))
}

/// `pg_wal_lsn_diff(a, b)`: the signed byte distance `a - b`.
fn lsn_diff(values: &[SqlValue]) -> Result<SqlValue> {
    let [left, right] = values else {
        return Err(Error::UnsupportedSql(
            "pg_wal_lsn_diff expects two LSNs".to_owned(),
        ));
    };
    if matches!(left, SqlValue::Null) || matches!(right, SqlValue::Null) {
        return Ok(SqlValue::Null);
    }
    let (left, right) = (parse_lsn(left)?, parse_lsn(right)?);
    let diff = i128::from(left) - i128::from(right);
    // PostgreSQL answers `numeric`; a distance beyond the i64 range is
    // refused rather than rounded.
    i64::try_from(diff)
        .map(SqlValue::Integer)
        .map_err(|_| Error::UnsupportedCapability {
            feature: "pg_wal_lsn_diff",
            detail: format!("the distance {diff} does not fit a bigint"),
        })
}

/// Parse `X/Y`: one to eight hex digits on each side of the slash, and
/// nothing else, as PostgreSQL's `pg_lsn_in` does.
fn parse_lsn(value: &SqlValue) -> Result<u64> {
    let invalid = |text: &str| {
        Error::UnsupportedSql(format!("invalid input syntax for type pg_lsn: \"{text}\""))
    };
    let text = match value {
        SqlValue::Text(text) => text.as_ref().to_owned(),
        SqlValue::Integer(n) => n.to_string(),
        SqlValue::Real(n) => n.to_string(),
        SqlValue::Blob(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        SqlValue::Null => unreachable!("NULL is handled by the caller"),
    };
    let half =
        |part: &str| (1..=8).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_hexdigit());
    let Some((high, low)) = text.split_once('/') else {
        return Err(invalid(&text));
    };
    if !half(high) || !half(low) {
        return Err(invalid(&text));
    }
    let high = u64::from_str_radix(high, 16).map_err(|_| invalid(&text))?;
    let low = u64::from_str_radix(low, 16).map_err(|_| invalid(&text))?;
    Ok((high << 32) | low)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> SqlValue {
        SqlValue::Text(Arc::from(value))
    }

    #[test]
    fn lsn_parsing_follows_pg_lsn_in() {
        assert_eq!(parse_lsn(&text("0/10")).unwrap(), 16);
        assert_eq!(parse_lsn(&text("1/0")).unwrap(), 1 << 32);
        assert_eq!(parse_lsn(&text("ffffffff/FFFFFFFF")).unwrap(), u64::MAX);
        for bad in [
            "",
            "/",
            "0/",
            "/0",
            "1/2/3",
            "123456789/0",
            "0/123456789",
            " 0/1",
            "0/1 ",
            "g/0",
            "+1/0",
        ] {
            let err = parse_lsn(&text(bad)).expect_err(bad).to_string();
            assert!(
                err.contains("invalid input syntax for type pg_lsn"),
                "{bad}: {err}"
            );
        }
    }

    #[test]
    fn lsn_diff_is_signed_and_refuses_what_a_bigint_cannot_hold() {
        let diff = |a: &str, b: &str| lsn_diff(&[text(a), text(b)]);
        assert_eq!(diff("0/10", "0/0").unwrap(), SqlValue::Integer(16));
        assert_eq!(diff("0/0", "0/10").unwrap(), SqlValue::Integer(-16));
        assert_eq!(
            diff("7FFFFFFF/FFFFFFFF", "0/0").unwrap(),
            SqlValue::Integer(i64::MAX)
        );
        let err = diff("FFFFFFFF/FFFFFFFF", "0/0").unwrap_err().to_string();
        assert!(
            err.contains("unsupported capability: pg_wal_lsn_diff"),
            "{err}"
        );
        let err = diff("0/0", "FFFFFFFF/FFFFFFFF").unwrap_err().to_string();
        assert!(
            err.contains("unsupported capability: pg_wal_lsn_diff"),
            "{err}"
        );
        assert_eq!(
            lsn_diff(&[SqlValue::Null, text("0/0")]).unwrap(),
            SqlValue::Null
        );
    }

    #[test]
    fn nothing_answers_outside_the_postgres_dialect() {
        // No dialect scope is installed on this thread: SQLite.
        for name in [
            "txid_current",
            "pg_notify",
            "repeat",
            "current_user",
            "pg_advisory_lock",
        ] {
            assert!(try_eval(name, &[]).is_none(), "{name}");
        }
    }
}
