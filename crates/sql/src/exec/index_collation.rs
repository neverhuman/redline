//! Whether an index key can answer a comparison, by collation (workplan
//! Q5-10).
//!
//! An index key stores text normalized by its collation (NOCASE folds case,
//! RTRIM drops trailing spaces), so it answers a comparison only when the
//! comparison uses that same collation; then the probe value is normalized
//! the same way. A BINARY `x = 'X'` must not probe a NOCASE key (it would
//! also find `'x'`), and a NOCASE `x = 'X'` must not probe a BINARY key (it
//! would miss `'x'`).

use redlinedb_kernel::catalog::{ColumnDef, IndexKeyDef, TableDef};
use sqlparser::ast::Expr;

use crate::collation::Collation;
use crate::exec::expr::coerce::{collation_from_expr, column_reference, declared_collation};
use crate::value::SqlValue;

fn or_binary(collation: Option<Collation>) -> Collation {
    collation.unwrap_or(Collation::Binary)
}

/// The collation `key` stores its text under.
pub(super) fn key_collation(key: &IndexKeyDef) -> Collation {
    or_binary(key.collation.as_deref().and_then(Collation::parse))
}

/// The collation of the comparison `left <op> right`, where one side is a
/// column of `table` (SQLite precedence: explicit left, explicit right,
/// then the declared collation of the left column, then the right's).
pub(super) fn predicate_collation(left: &Expr, right: &Expr, table: &TableDef) -> Collation {
    let column = |expr: &Expr| {
        let (_, name) = column_reference(expr)?;
        declared_collation(table, name)
    };
    or_binary(
        collation_from_expr(left)
            .or_else(|| collation_from_expr(right))
            .or_else(|| column(left))
            .or_else(|| column(right)),
    )
}

/// `key` can answer `left <op> right` exactly.
pub(super) fn key_answers(key: &IndexKeyDef, left: &Expr, right: &Expr, table: &TableDef) -> bool {
    key_collation(key) == predicate_collation(left, right, table)
}

/// A probe value in the form `key` stores it.
pub(super) fn probe_value(value: SqlValue, key: &IndexKeyDef) -> SqlValue {
    super::index_dml::apply_index_key_collation(value, key.collation.as_deref())
}

/// An index key whose collation normalizes text cannot give back the
/// stored value, so a covering read of it must go to the heap.
pub(crate) fn key_normalizes_text(key: &IndexKeyDef) -> bool {
    matches!(key_collation(key), Collation::NoCase | Collation::RTrim)
}

/// `ORDER BY col` sorts under the column's collation; an index walks in its
/// key's, so it gives that order only when the two are the same.
pub(super) fn key_orders_like_column(key: &IndexKeyDef, column: &ColumnDef) -> bool {
    redlinedb_kernel::catalog::collation::same_collation(
        key.collation.as_deref(),
        column.collation.as_deref(),
    )
}
