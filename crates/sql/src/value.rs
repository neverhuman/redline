use std::cell::Cell;
use std::cmp::Ordering;
use std::sync::Arc;

use crate::connection::Dialect;

#[allow(unused_imports)]
pub use redlinedb_kernel::catalog::{
    Affinity, EvalScratch, ExprAst, OwnedValue, RecordRef, RecordScratch, StorageClass, ValueRef,
    apply_affinity, derive_affinity, encode_record,
};

pub type SqlValue = OwnedValue;
pub type SqlValueRef<'a> = ValueRef<'a>;

fn compare_text_maybe_citext(left: &str, right: &str) -> Ordering {
    let (left_ci, left_text) = split_citext(left);
    let (right_ci, right_text) = split_citext(right);
    if left_ci || right_ci {
        return left_text
            .to_ascii_lowercase()
            .cmp(&right_text.to_ascii_lowercase());
    }
    left.cmp(right)
}

fn split_citext(text: &str) -> (bool, &str) {
    match text.strip_prefix('\u{E000}') {
        Some(rest) => (true, rest),
        None => (false, text),
    }
}

pub fn compare_values(left: &SqlValue, right: &SqlValue) -> Ordering {
    use OwnedValue::*;
    match (left, right) {
        (Null, Null) => Ordering::Equal,
        (Null, _) => Ordering::Less,
        (_, Null) => Ordering::Greater,
        (Integer(a), Integer(b)) => a.cmp(b),
        (Real(a), Real(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
        // Exact, as SQLite's sqlite3IntFloatCompare: `as f64` is lossy above 2^53.
        (Integer(a), Real(b)) => crate::numeric::int_real_cmp(*a, *b),
        (Real(a), Integer(b)) => crate::numeric::int_real_cmp(*b, *a).reverse(),
        (Integer(_) | Real(_), Text(_) | Blob(_)) => Ordering::Less,
        (Text(_) | Blob(_), Integer(_) | Real(_)) => Ordering::Greater,
        (Text(a), Text(b)) => compare_text_maybe_citext(a, b),
        (Blob(a), Blob(b)) => a.as_ref().cmp(b.as_ref()),
        (Text(_), Blob(_)) => Ordering::Less,
        (Blob(_), Text(_)) => Ordering::Greater,
    }
}

const THREAD_DIALECT_UNSET: u8 = 0;
const THREAD_DIALECT_SQLITE: u8 = 1;
const THREAD_DIALECT_POSTGRES: u8 = 2;

thread_local! {
    /// The dialect of the connection whose statement this thread is running.
    /// Installed by `exec::with_current_connection` (every prepare and step);
    /// a thread outside any statement reads as the SQLite dialect. SQL
    /// evaluation does not leave the statement's thread: the parallel heap
    /// scan runs kernel code only, and a parallel sort compares values
    /// without reading the dialect.
    static THREAD_DIALECT: Cell<u8> = const { Cell::new(THREAD_DIALECT_UNSET) };
}

/// Restores the previous thread dialect when dropped, so a panic that
/// unwinds out of a statement does not leave the slot set.
pub(crate) struct DialectScope {
    previous: u8,
}

impl DialectScope {
    fn enter(state: u8) -> Self {
        Self {
            previous: THREAD_DIALECT.with(|cell| cell.replace(state)),
        }
    }

    pub(crate) fn for_dialect(dialect: Dialect) -> Self {
        Self::enter(if dialect.is_postgres() {
            THREAD_DIALECT_POSTGRES
        } else {
            THREAD_DIALECT_SQLITE
        })
    }
}

impl Drop for DialectScope {
    fn drop(&mut self) {
        THREAD_DIALECT.with(|cell| cell.set(self.previous));
    }
}

/// True while this thread runs a statement for a connection whose database
/// speaks [`Dialect::PostgresSubset`]: boolean results render as `t`/`f` and
/// `::text` of a boolean renders as `true`/`false`. The dialect is a
/// per-database option ([`crate::DbOptions::dialect`]); this reads a
/// thread-local slot and never the environment.
pub fn postgres_result_dialect() -> bool {
    THREAD_DIALECT.with(Cell::get) == THREAD_DIALECT_POSTGRES
}

/// Boolean result for the active result dialect: `t`/`f` under Postgres, else `1`/`0`.
pub fn postgres_bool(yes: bool) -> SqlValue {
    if postgres_result_dialect() {
        SqlValue::Text(Arc::from(if yes { "t" } else { "f" }))
    } else {
        SqlValue::Integer(if yes { 1 } else { 0 })
    }
}

pub fn is_truthy(value: &SqlValue) -> bool {
    if let OwnedValue::Text(text) = value
        && postgres_result_dialect()
    {
        let trimmed = text.as_ref().trim();
        if trimmed.eq_ignore_ascii_case("t") || trimmed.eq_ignore_ascii_case("true") {
            return true;
        }
        if trimmed.eq_ignore_ascii_case("f") || trimmed.eq_ignore_ascii_case("false") {
            return false;
        }
    }
    match value {
        OwnedValue::Null => false,
        OwnedValue::Integer(v) => *v != 0,
        OwnedValue::Real(v) => *v != 0.0,
        // SQLite reads TEXT and BLOB through their longest numeric prefix
        // (`sqlite3VdbeBooleanValue` -> `sqlite3AtoF`): `'1abc'`, `x'31ff'`,
        // `'1e'` and `'.5x'` are true; `'abc'`, `'inf'` and `'nan'` are false.
        OwnedValue::Text(v) => crate::numeric::sqlite_text_is_true(v.as_bytes()),
        OwnedValue::Blob(v) => crate::numeric::sqlite_text_is_true(v),
    }
}

pub fn canonicalize(value: SqlValue) -> SqlValue {
    match value {
        OwnedValue::Real(0.0) => OwnedValue::Real(0.0),
        OwnedValue::Real(v) if v.is_nan() => OwnedValue::Real(f64::NAN),
        other => other,
    }
}

#[allow(dead_code)]
pub fn text_value(value: impl Into<Arc<str>>) -> SqlValue {
    OwnedValue::Text(value.into())
}
