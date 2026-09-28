//! SQLite comparison affinity (https://sqlite.org/datatype3.html §4.2).
//!
//! Before `=`, `<`, `IN`, `BETWEEN`, `IS [NOT] DISTINCT FROM` and
//! `CASE x WHEN` compare two values, SQLite converts them by the affinity
//! of the operand *expressions* (`sqlite3CompareAffinity`):
//!
//! * A column has its declared affinity (a column without a type has BLOB
//!   affinity, which is still "an affinity"); `CAST(x AS T)` has T's
//!   affinity; a scalar subquery has its first result column's; everything
//!   else (literals, parameters, functions, `+x`) has none.
//! * If both operands have an affinity, the comparison is NUMERIC when
//!   either is INTEGER, REAL or NUMERIC, otherwise it converts nothing.
//! * If one operand has an affinity, the comparison uses it.
//! * NUMERIC turns a TEXT operand that is a well-formed number into that
//!   number; TEXT turns a number into its text. Nothing is converted unless
//!   an operand is TEXT.
//!
//! So `x = '5'` finds INTEGER 5 in an INTEGER column, `y = 5` finds TEXT '5'
//! in a TEXT column, and `z = '5'` does not find 5 in a column without a
//! type. `x IN (...)` uses the left operand's affinity alone.
//!
//! Index probes follow `sqlite3IndexAffinityOk`: an index can serve the
//! comparison only when the comparison converts the probe the way the
//! column's own affinity would ([`index_usable`]).

use std::sync::Arc;

use redlinedb_kernel::catalog::{Affinity, TableDef};
use sqlparser::ast::{Expr, SelectItem};

use crate::exec::expr::scalar::row::RowContext;
use crate::statement::{SelectPlan, SelectSource};
use crate::value::SqlValue;

/// What a comparison converts its operands to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CmpAffinity {
    /// No conversion (both operands without affinity, or two non-numeric
    /// affinities, or a column without a type against an expression).
    None,
    /// Numbers become text.
    Text,
    /// Well-formed numeric text becomes a number.
    Numeric,
}

fn is_numeric(affinity: Affinity) -> bool {
    matches!(
        affinity,
        Affinity::Integer | Affinity::Real | Affinity::Numeric
    )
}

impl CmpAffinity {
    /// The conversion a single operand with `affinity` imposes when the
    /// other operand has none (and the conversion an index on a column
    /// with that affinity applies to a constant probe).
    pub(crate) fn of(affinity: Affinity) -> Self {
        match affinity {
            Affinity::Text => Self::Text,
            Affinity::Blob => Self::None,
            _ => Self::Numeric,
        }
    }

    /// `sqlite3CompareAffinity` over the two operands' affinities.
    pub(crate) fn between(left: Option<Affinity>, right: Option<Affinity>) -> Self {
        match (left, right) {
            (Some(a), Some(b)) => {
                if is_numeric(a) || is_numeric(b) {
                    Self::Numeric
                } else {
                    Self::None
                }
            }
            (Some(a), None) | (None, Some(a)) => Self::of(a),
            (None, None) => Self::None,
        }
    }

    /// `x IN (list)` compares with the left operand's affinity alone.
    pub(crate) fn of_optional(affinity: Option<Affinity>) -> Self {
        affinity.map_or(Self::None, Self::of)
    }
}

/// Cheap pre-check: can any comparison affinity change how these two
/// values compare? Only when a TEXT meets a number, or when a TEXT that
/// could be a number meets another TEXT. Everything else compares the
/// same whatever the affinity, so the caller skips the affinity lookup.
pub(crate) fn may_convert(left: &SqlValue, right: &SqlValue) -> bool {
    match (left, right) {
        (SqlValue::Text(_), SqlValue::Integer(_) | SqlValue::Real(_))
        | (SqlValue::Integer(_) | SqlValue::Real(_), SqlValue::Text(_)) => true,
        (SqlValue::Text(a), SqlValue::Text(b)) => could_be_number(a) || could_be_number(b),
        _ => false,
    }
}

fn could_be_number(text: &str) -> bool {
    text.bytes()
        .find(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))
        .is_some_and(|byte| byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.'))
}

/// Convert both operands the way SQLite's comparison opcodes do. Nothing
/// changes unless one of them is TEXT.
pub(crate) fn apply_pair(
    left: SqlValue,
    right: SqlValue,
    affinity: CmpAffinity,
) -> (SqlValue, SqlValue) {
    if affinity == CmpAffinity::None
        || !(matches!(left, SqlValue::Text(_)) || matches!(right, SqlValue::Text(_)))
    {
        return (left, right);
    }
    (apply_one(left, affinity), apply_one(right, affinity))
}

/// Convert one value (an index probe, whose partner is the stored key).
pub(crate) fn apply_one(value: SqlValue, affinity: CmpAffinity) -> SqlValue {
    match (affinity, value) {
        (CmpAffinity::Numeric, SqlValue::Text(text)) => {
            crate::numeric::text_number::comparison_number(&text).unwrap_or(SqlValue::Text(text))
        }
        (CmpAffinity::Text, SqlValue::Integer(v)) => SqlValue::Text(Arc::from(v.to_string())),
        (CmpAffinity::Text, SqlValue::Real(v)) => SqlValue::Text(Arc::from(
            crate::exec::expr::scalar::value::format_real_sqlite(v),
        )),
        (_, value) => value,
    }
}

/// `sqlite3IndexAffinityOk` for a constant (or other-table) probe with
/// affinity `probe` against an index column with affinity `column`. A
/// comparison that converts nothing can always use the index (two TEXT
/// columns, for one); a TEXT comparison needs a TEXT index and a NUMERIC
/// one a numeric index, whose keys hold the values that conversion makes.
///
/// A probe the index path converts by the column's affinity where the
/// comparison converts nothing must be one that conversion leaves alone;
/// [`probe_unchanged_by_index`] checks that.
pub(crate) fn index_usable(column: Affinity, probe: Option<Affinity>) -> bool {
    match CmpAffinity::between(Some(column), probe) {
        CmpAffinity::None => true,
        CmpAffinity::Text => column == Affinity::Text,
        CmpAffinity::Numeric => is_numeric(column),
    }
}

/// Whether converting `value` by the index column's affinity, as an index
/// probe does, keeps it as it is (same storage class, same value).
pub(crate) fn probe_unchanged_by_index(column: Affinity, value: &SqlValue) -> bool {
    let converted = apply_one(value.clone(), CmpAffinity::of(column));
    match (&converted, value) {
        (SqlValue::Integer(a), SqlValue::Integer(b)) => a == b,
        (SqlValue::Real(a), SqlValue::Real(b)) => a.to_bits() == b.to_bits(),
        (SqlValue::Text(a), SqlValue::Text(b)) => a == b,
        (SqlValue::Blob(a), SqlValue::Blob(b)) => a == b,
        (SqlValue::Null, SqlValue::Null) => true,
        _ => false,
    }
}

/// The affinity `CAST(x AS type_name)` gives its value.
pub(crate) fn cast_affinity(type_name: &str) -> Affinity {
    redlinedb_kernel::catalog::derive_affinity(Some(type_name))
}

/// `sqlite3ExprAffinity` of `expr` evaluated against `row` (and, for a
/// correlated reference, the enclosing rows). `None` means no affinity.
pub(crate) fn expr_affinity(row: &RowContext<'_>, expr: &Expr) -> Option<Affinity> {
    match expr {
        Expr::Nested(inner) => expr_affinity(row, inner),
        Expr::Collate { expr, .. } => expr_affinity(row, expr),
        Expr::Cast { data_type, .. } => Some(cast_affinity(&data_type.to_string())),
        Expr::Identifier(ident) => column_affinity_in_scope(row, None, &ident.value),
        Expr::CompoundIdentifier(parts) if parts.len() >= 2 => {
            let qualifier = &parts[parts.len() - 2].value;
            let name = &parts[parts.len() - 1].value;
            column_affinity_in_scope(row, Some(qualifier), name)
        }
        Expr::Subquery(query) => super::predicate::subquery_column_affinity(query, 0),
        _ => None,
    }
}

/// `Some(found)` when the name resolves in `row` (`found` may still be
/// "no affinity"), `None` when it does not resolve there.
type Resolved = Option<Option<Affinity>>;

fn column_affinity_in_scope(
    row: &RowContext<'_>,
    qualifier: Option<&str>,
    name: &str,
) -> Option<Affinity> {
    if let Some(found) = column_affinity_local(row, qualifier, name, false) {
        return found;
    }
    crate::exec::lookup_correlated(|outer| column_affinity_local(outer, qualifier, name, true))
        .flatten()
}

/// The affinity of `name` in `table`: a declared column, else the rowid.
///
/// Tables synthesized for views, CTEs, FROM-subqueries and attached-table
/// aliases carry affinities guessed from their first row; SQLite gives
/// those columns the affinity of their defining expression instead, which
/// is not tracked yet, so they report none (the behaviour before
/// comparison affinity existed).
pub(crate) fn table_column_affinity(table: &TableDef, name: &str) -> Option<Affinity> {
    if crate::exec::cte::is_cte_table_def(table) {
        return None;
    }
    if let Some(column) = table
        .columns
        .iter()
        .find(|column| column.folded.as_ref().eq_ignore_ascii_case(name))
    {
        return Some(column.affinity);
    }
    table
        .is_public_rowid_name(name)
        .then_some(Affinity::Integer)
}

fn names_table(alias: Option<&Arc<str>>, table: &TableDef, qualifier: &str) -> bool {
    match alias {
        Some(alias) => alias.as_ref().eq_ignore_ascii_case(qualifier),
        None => table.name.eq_ignore_ascii_case(qualifier),
    }
}

fn resolves(table: &TableDef, name: &str) -> bool {
    table
        .columns
        .iter()
        .any(|column| column.folded.as_ref().eq_ignore_ascii_case(name))
        || table.is_public_rowid_name(name)
}

/// A trigger's `NEW` and `OLD` rows sit on the correlated-row stack under
/// exactly these aliases (`exec::trigger::make_table_row`).
fn is_trigger_row(alias: Option<&Arc<str>>) -> bool {
    alias.is_some_and(|alias| matches!(alias.as_ref(), "NEW" | "OLD"))
}

/// SQLite resolves `NEW.col`/`OLD.col` to `TK_TRIGGER`, which has no
/// affinity; only the rowid (and an INTEGER PRIMARY KEY) is INTEGER. So
/// `WHEN new.x = '9'` does not fire for x = 9 even in an INTEGER column.
fn trigger_column_affinity(table: &TableDef, name: &str) -> Option<Affinity> {
    let rowid = table.rowid_alias_column_name_matches(name)
        || (table.is_public_rowid_name(name)
            && !table
                .columns
                .iter()
                .any(|column| column.folded.as_ref().eq_ignore_ascii_case(name)));
    rowid.then_some(Affinity::Integer)
}

fn column_affinity_local(
    row: &RowContext<'_>,
    qualifier: Option<&str>,
    name: &str,
    correlated: bool,
) -> Resolved {
    match row {
        RowContext::Table(row) => match qualifier {
            Some(q) if !names_table(row.alias.as_ref(), &row.table, q) => None,
            _ if !resolves(&row.table, name) => None,
            _ if correlated && is_trigger_row(row.alias.as_ref()) => {
                Some(trigger_column_affinity(&row.table, name))
            }
            _ => Some(table_column_affinity(&row.table, name)),
        },
        RowContext::Upsert { current, .. } => match qualifier {
            Some(q)
                if !q.eq_ignore_ascii_case("excluded")
                    && !names_table(current.alias.as_ref(), &current.table, q) =>
            {
                None
            }
            _ if !resolves(&current.table, name) => None,
            _ => Some(table_column_affinity(&current.table, name)),
        },
        RowContext::Joined(rows) => rows.iter().find_map(|joined| match qualifier {
            Some(q) if !names_table(joined.alias.as_ref(), &joined.table, q) => None,
            Some(_) if !resolves(&joined.table, name) => None,
            None if joined.hides_column_name(name) || !resolves(&joined.table, name) => None,
            _ => Some(table_column_affinity(&joined.table, name)),
        }),
        RowContext::Cte(row) => {
            let qualified_here = qualifier.is_none_or(|q| {
                row.alias
                    .as_deref()
                    .is_some_and(|alias| alias.eq_ignore_ascii_case(q))
                    || row.name.as_ref().eq_ignore_ascii_case(q)
            });
            (qualified_here && row.columns.iter().any(|c| c.eq_ignore_ascii_case(name)))
                .then_some(None)
        }
        _ => None,
    }
}

/// The affinity of result column `index` of a SELECT, resolved against the
/// tables it reads (`sqlite3ExprAffinity` of that result expression).
/// Compound selects, CTE sources and unresolvable expressions give `None`.
pub(crate) fn plan_column_affinity(plan: &SelectPlan, index: usize) -> Option<Affinity> {
    let tables: Vec<(Option<&Arc<str>>, &Arc<TableDef>)> = match &plan.source {
        SelectSource::Table(table) => vec![(None, table)],
        SelectSource::Tables(bound) => bound.iter().map(|b| (b.alias.as_ref(), &b.table)).collect(),
        SelectSource::Joined(join) => std::iter::once((join.base.alias.as_ref(), &join.base.table))
            .chain(
                join.joins
                    .iter()
                    .map(|step| (step.right.alias.as_ref(), &step.right.table)),
            )
            .collect(),
        _ => return None,
    };
    let expr = match plan.projection.get(index) {
        Some(SelectItem::UnnamedExpr(expr)) | Some(SelectItem::ExprWithAlias { expr, .. }) => expr,
        Some(SelectItem::Wildcard(_)) if plan.projection.len() == 1 && tables.len() == 1 => {
            return tables[0].1.columns.get(index).map(|column| column.affinity);
        }
        None if plan.projection.is_empty() && tables.len() == 1 => {
            return tables[0].1.columns.get(index).map(|column| column.affinity);
        }
        _ => return None,
    };
    static_expr_affinity(expr, &tables)
}

fn static_expr_affinity(
    expr: &Expr,
    tables: &[(Option<&Arc<str>>, &Arc<TableDef>)],
) -> Option<Affinity> {
    match expr {
        Expr::Nested(inner) => static_expr_affinity(inner, tables),
        Expr::Collate { expr, .. } => static_expr_affinity(expr, tables),
        Expr::Cast { data_type, .. } => Some(cast_affinity(&data_type.to_string())),
        Expr::Identifier(ident) => tables
            .iter()
            .find_map(|(_, table)| table_column_affinity(table, &ident.value)),
        Expr::CompoundIdentifier(parts) if parts.len() >= 2 => {
            let qualifier = &parts[parts.len() - 2].value;
            let name = &parts[parts.len() - 1].value;
            tables
                .iter()
                .filter(|(alias, table)| names_table(*alias, table, qualifier))
                .find_map(|(_, table)| table_column_affinity(table, name))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(v: &str) -> SqlValue {
        SqlValue::Text(Arc::from(v))
    }

    #[test]
    fn compare_affinity_follows_sqlite3_compare_affinity() {
        use Affinity::*;
        assert_eq!(
            CmpAffinity::between(Some(Integer), None),
            CmpAffinity::Numeric
        );
        assert_eq!(CmpAffinity::between(None, Some(Text)), CmpAffinity::Text);
        assert_eq!(CmpAffinity::between(Some(Blob), None), CmpAffinity::None);
        assert_eq!(
            CmpAffinity::between(Some(Text), Some(Integer)),
            CmpAffinity::Numeric
        );
        assert_eq!(
            CmpAffinity::between(Some(Text), Some(Text)),
            CmpAffinity::None
        );
        assert_eq!(
            CmpAffinity::between(Some(Text), Some(Blob)),
            CmpAffinity::None
        );
        assert_eq!(CmpAffinity::between(None, None), CmpAffinity::None);
    }

    #[test]
    fn apply_pair_converts_only_when_text_is_involved() {
        let (l, r) = apply_pair(SqlValue::Integer(5), text("5"), CmpAffinity::Numeric);
        assert_eq!((l, r), (SqlValue::Integer(5), SqlValue::Integer(5)));
        let (l, r) = apply_pair(text("5"), SqlValue::Integer(5), CmpAffinity::Text);
        assert_eq!((l, r), (text("5"), text("5")));
        // Two numbers are never stringified, even under TEXT affinity.
        let (l, r) = apply_pair(
            SqlValue::Integer(10),
            SqlValue::Integer(9),
            CmpAffinity::Text,
        );
        assert_eq!((l, r), (SqlValue::Integer(10), SqlValue::Integer(9)));
        let (l, r) = apply_pair(text("5x"), SqlValue::Integer(5), CmpAffinity::Numeric);
        assert_eq!((l, r), (text("5x"), SqlValue::Integer(5)));
    }

    #[test]
    fn index_usable_matches_sqlite3_index_affinity_ok() {
        use Affinity::*;
        assert!(index_usable(Integer, None));
        assert!(index_usable(Integer, Some(Text)));
        assert!(index_usable(Text, None));
        assert!(!index_usable(Text, Some(Integer)));
        // Two non-numeric affinities compare as BLOB: nothing converts, and
        // `aff < SQLITE_AFF_TEXT` lets the index answer.
        assert!(index_usable(Text, Some(Text)));
        assert!(index_usable(Text, Some(Blob)));
        assert!(!index_usable(Blob, Some(Integer)));
        assert!(index_usable(Real, Some(Integer)));
        assert!(index_usable(Blob, None));
        assert!(index_usable(Blob, Some(Text)));
        assert!(!index_usable(Blob, Some(Real)));
    }

    #[test]
    fn may_convert_skips_pairs_no_affinity_can_change() {
        assert!(may_convert(&text("5"), &SqlValue::Integer(5)));
        assert!(may_convert(&text(" -1"), &text("abc")));
        assert!(!may_convert(&text("abc"), &text("def")));
        assert!(!may_convert(&SqlValue::Integer(1), &SqlValue::Real(1.0)));
        assert!(!may_convert(&SqlValue::Null, &text("5")));
        assert!(!may_convert(
            &text("5"),
            &SqlValue::Blob(Arc::from(&b"5"[..]))
        ));
    }
}
