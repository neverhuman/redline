use std::cell::Cell;
use std::cmp::Ordering;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use crate::connection::Dialect;

#[allow(unused_imports)]
pub use redlinedb_kernel::catalog::{
    Affinity, EvalScratch, ExprAst, OwnedValue, RecordRef, RecordScratch, StorageClass, ValueRef,
    apply_affinity, derive_affinity, encode_record,
};

pub type SqlValue = OwnedValue;
pub type SqlValueRef<'a> = ValueRef<'a>;

/// Set (and never cleared) the first time a Postgres-dialect connection runs
/// `CREATE EXTENSION citext`. Until then no value in the process can carry
/// the `::citext` marker, and a leading U+E000 is an ordinary character.
static CITEXT_MARKER_ENABLED: AtomicBool = AtomicBool::new(false);

/// PG-03: record that `::citext` values may exist in this process.
pub(crate) fn enable_citext_marker() {
    CITEXT_MARKER_ENABLED.store(true, AtomicOrdering::Relaxed);
}

/// True once some Postgres-dialect connection in this process has enabled
/// citext. The shell uses it (with its own dialect) to decide whether a
/// leading U+E000 is a marker to hide or a character to print.
pub fn citext_marker_enabled() -> bool {
    CITEXT_MARKER_ENABLED.load(AtomicOrdering::Relaxed)
}

/// PG-03: whether a leading U+E000 means "compare without case" for the
/// statement running on this thread. It does only under the Postgres
/// dialect, and only after citext was enabled; a SQLite-dialect connection,
/// or a thread outside any statement, compares U+E000 as the character it is.
pub(crate) fn citext_marker_active() -> bool {
    citext_marker_enabled() && postgres_result_dialect()
}

fn compare_text_maybe_citext(left: &str, right: &str) -> Ordering {
    if !citext_marker_active() {
        return left.cmp(right);
    }
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
    /// evaluation does not leave the statement's thread except for the
    /// parallel sort's comparisons, which carry it with [`DialectState`]
    /// (the parallel heap scan runs kernel code only).
    static THREAD_DIALECT: Cell<u8> = const { Cell::new(THREAD_DIALECT_UNSET) };
}

/// A copy of the calling thread's dialect slot, for a worker thread that
/// compares values on behalf of a statement (a parallel sort).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DialectState(u8);

impl DialectState {
    pub(crate) fn current() -> Self {
        Self(THREAD_DIALECT.with(Cell::get))
    }

    /// Run `f` with this state installed on the calling thread.
    pub(crate) fn install<T>(self, f: impl FnOnce() -> T) -> T {
        let _scope = DialectScope::enter(self.0);
        f()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text(v: &str) -> SqlValue {
        text_value(v)
    }

    #[test]
    fn a_leading_private_use_character_is_plain_text_outside_postgres_citext() {
        let marked = text("\u{E000}A");
        assert_eq!(compare_values(&marked, &text("\u{E000}A")), Ordering::Equal);
        assert_ne!(compare_values(&marked, &text("a")), Ordering::Equal);
        // Enabling citext somewhere in the process does not change a thread
        // that is not running a Postgres-dialect statement.
        enable_citext_marker();
        assert_ne!(compare_values(&marked, &text("a")), Ordering::Equal);
        {
            let _sqlite = DialectScope::for_dialect(Dialect::Sqlite);
            assert_ne!(compare_values(&marked, &text("a")), Ordering::Equal);
        }
        let _postgres = DialectScope::for_dialect(Dialect::PostgresSubset);
        assert_eq!(compare_values(&marked, &text("a")), Ordering::Equal);
    }
}
