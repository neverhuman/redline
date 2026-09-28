//! Q5-05: Postgres value semantics need Postgres provenance.
//!
//! `||`, `-`, `+`, `*`, `/` and `%` used to switch to jsonb merge, jsonb key
//! delete, date subtraction, exact decimal arithmetic or trigram similarity
//! whenever a TEXT operand merely looked like JSON, a date or a decimal. In
//! the SQLite dialect `'[1]'||'[2]'` then answered `[1, 2]` instead of
//! `[1][2]`, `'2025-01-02'-'2025-01-01'` answered `1 day` instead of 0 and
//! `'7'%'4'` answered 0 instead of 3.
//!
//! Those readings now need either the Postgres dialect or an operand whose
//! expression says it is a Postgres value: a `::` cast to a numeric, date,
//! time-stamp, interval or JSON type, a function that returns jsonb, or an
//! operator applied to such an operand. The constant folder keeps those
//! casts (see `parser::select::expr_is_foldable`) so the provenance is still
//! in the tree when the operator runs.

use sqlparser::ast::{CastKind, DataType, Expr};

/// Type names whose `::` cast marks a value as a Postgres value.
fn is_pg_value_type(type_name: &str) -> bool {
    let lower = type_name.trim().to_ascii_lowercase();
    let base = lower.split('(').next().unwrap_or(&lower).trim();
    matches!(
        base,
        "numeric"
            | "decimal"
            | "date"
            | "interval"
            | "json"
            | "jsonb"
            | "timestamp"
            | "timestamptz"
            | "timestamp with time zone"
            | "timestamp without time zone"
    )
}

/// `expr` is a `::` cast to a Postgres value type. Such a cast must not be
/// folded into a literal: the literal would lose the provenance.
pub(crate) fn is_pg_value_cast(kind: &CastKind, data_type: &DataType) -> bool {
    matches!(kind, CastKind::DoubleColon) && is_pg_value_type(&data_type.to_string())
}

/// Functions that return a jsonb value under their Postgres name. SQLite's
/// own `jsonb()` returns a BLOB and is not one of them.
fn returns_jsonb(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "to_jsonb"
            | "jsonb_build_object"
            | "jsonb_build_array"
            | "jsonb_set"
            | "jsonb_insert"
            | "jsonb_strip_nulls"
            | "jsonb_delete"
            | "jsonb_delete_path"
            | "jsonb_concat"
            | "jsonb_path_query_first"
    )
}

/// The expression's own syntax marks its value as a Postgres value.
pub(crate) fn has_pg_provenance(expr: &Expr) -> bool {
    match expr {
        Expr::Nested(inner) => has_pg_provenance(inner),
        Expr::Cast {
            kind, data_type, ..
        } => is_pg_value_cast(kind, data_type),
        Expr::Function(func) => {
            let name = func.name.to_string();
            returns_jsonb(name.rsplit('.').next().unwrap_or(&name))
        }
        Expr::BinaryOp { left, right, .. } => has_pg_provenance(left) || has_pg_provenance(right),
        Expr::UnaryOp { expr, .. } => has_pg_provenance(expr),
        _ => false,
    }
}

/// Postgres semantics apply to an operator over `left` and `right` when the
/// dialect is Postgres or either operand has Postgres provenance.
pub(crate) fn pg_semantics(left: &Expr, right: &Expr) -> bool {
    crate::value::postgres_result_dialect() || has_pg_provenance(left) || has_pg_provenance(right)
}

/// Date subtraction needs a Postgres value on both sides (or the Postgres
/// dialect): `'2025-01-02'::date - '2025-01-01'` is still SQLite arithmetic.
pub(crate) fn pg_semantics_both(left: &Expr, right: &Expr) -> bool {
    crate::value::postgres_result_dialect() || (has_pg_provenance(left) && has_pg_provenance(right))
}
