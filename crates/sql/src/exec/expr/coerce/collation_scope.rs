//! Which collation a comparison, a sort key or a grouping key uses
//! (workplan Q5-10).
//!
//! SQLite's rules: an explicit postfix `COLLATE` wins, the left operand's
//! before the right's; otherwise a column operand compares with its
//! declared collation, again the left one's first; otherwise BINARY. A
//! column wrapped in unary `+` or `CAST` still counts as the column. The
//! declared collation comes from the catalog (`ColumnDef::collation`), not
//! from the table's SQL text, so a quoted column name or a comma in a type
//! cannot hide it.

use std::cmp::Ordering;

use redlinedb_kernel::catalog::TableDef;
use sqlparser::ast::{Expr, UnaryOperator};

use super::super::scalar::RowContext;
use super::collation_from_expr;
use crate::collation::Collation;
use crate::value::SqlValue;

/// The collation of a comparison between `left` and `right`.
pub(crate) fn comparison_collation(
    left: &Expr,
    right: &Expr,
    row: &RowContext<'_>,
) -> Option<Collation> {
    collation_from_expr(left)
        .or_else(|| collation_from_expr(right))
        .or_else(|| column_collation(row, left))
        .or_else(|| column_collation(row, right))
}

/// The collation `expr` carries on its own: explicit, else its column's.
pub(crate) fn expr_collation(expr: &Expr, row: &RowContext<'_>) -> Option<Collation> {
    collation_from_expr(expr).or_else(|| column_collation(row, expr))
}

/// `compare_values`, but text compares under `collation`.
pub(crate) fn compare_with_collation(
    left: &SqlValue,
    right: &SqlValue,
    collation: Option<&Collation>,
) -> Ordering {
    collation
        .and_then(|collation| collation.compare_values(left, right))
        .unwrap_or_else(|| crate::value::compare_values(left, right))
}

/// The key a value groups and deduplicates under: text folded the way
/// `collation` compares it, so equal-under-collation values share a key.
pub(crate) fn collation_key(value: SqlValue, collation: Option<&Collation>) -> SqlValue {
    match (value, collation) {
        (SqlValue::Text(text), Some(collation @ (Collation::NoCase | Collation::RTrim))) => {
            let folded = collation.sort_text(&text);
            if folded.len() == text.len() && *folded == *text {
                SqlValue::Text(text)
            } else {
                SqlValue::Text(folded.into())
            }
        }
        (value, _) => value,
    }
}

/// The declared collation of the column `expr` names, when `expr` is a
/// column reference (through parentheses, unary `+` and `CAST`).
pub(crate) fn column_collation(row: &RowContext<'_>, expr: &Expr) -> Option<Collation> {
    let (qualifier, name) = column_reference(expr)?;
    let declared = |table: &TableDef| declared_collation(table, name);
    match row {
        RowContext::Table(table_row) => {
            if qualifier
                .is_some_and(|q| !names_table(&table_row.table, table_row.alias.as_deref(), q))
            {
                return None;
            }
            declared(&table_row.table)
        }
        RowContext::Upsert { current, .. } => {
            // `excluded.x` names the same column as `x`.
            if qualifier.is_some_and(|q| {
                !q.eq_ignore_ascii_case("excluded")
                    && !names_table(&current.table, current.alias.as_deref(), q)
            }) {
                return None;
            }
            declared(&current.table)
        }
        RowContext::Joined(rows) => rows
            .iter()
            .find(|joined| {
                qualifier.is_none_or(|q| names_table(&joined.table, joined.alias.as_deref(), q))
                    && has_column(&joined.table, name)
            })
            .and_then(|joined| declared(&joined.table)),
        _ => None,
    }
}

/// The declared collation of column `name` of `table`.
pub(crate) fn declared_collation(table: &TableDef, name: &str) -> Option<Collation> {
    let column = table
        .columns
        .iter()
        .find(|column| column.folded.eq_ignore_ascii_case(name))?;
    column.collation.as_deref().and_then(Collation::parse)
}

fn has_column(table: &TableDef, name: &str) -> bool {
    table
        .columns
        .iter()
        .any(|column| column.folded.eq_ignore_ascii_case(name))
}

fn names_table(table: &TableDef, alias: Option<&str>, qualifier: &str) -> bool {
    match alias {
        Some(alias) => alias.eq_ignore_ascii_case(qualifier),
        None => table.name.eq_ignore_ascii_case(qualifier),
    }
}

/// `(qualifier, column)` of a column reference.
pub(crate) fn column_reference(expr: &Expr) -> Option<(Option<&str>, &str)> {
    match expr {
        Expr::Identifier(ident) => Some((None, ident.value.as_str())),
        Expr::CompoundIdentifier(parts) if parts.len() >= 2 => Some((
            Some(parts[parts.len() - 2].value.as_str()),
            parts[parts.len() - 1].value.as_str(),
        )),
        Expr::Nested(inner) => column_reference(inner),
        Expr::UnaryOp {
            op: UnaryOperator::Plus,
            expr,
        } => column_reference(expr),
        Expr::Cast { expr, .. } => column_reference(expr),
        _ => None,
    }
}

/// The collation of each result column of `plan`, for DISTINCT and the
/// compound operators: an explicit COLLATE on the projection item, else
/// the declared collation of the column it names. A compound takes each
/// column's collation from its left-most branch that has one, as SQLite's
/// `multiSelectCollSeq` does.
pub(crate) fn plan_output_collations(
    plan: &crate::statement::SelectPlan,
) -> Vec<Option<Collation>> {
    use crate::statement::SelectSource;
    use sqlparser::ast::SelectItem;

    if let SelectSource::CompoundSet { branches, .. } | SelectSource::CompoundAll(branches) =
        &plan.source
    {
        let mut out: Vec<Option<Collation>> = Vec::new();
        for branch in branches {
            let collations = plan_output_collations(branch);
            if out.len() < collations.len() {
                out.resize(collations.len(), None);
            }
            for (slot, collation) in out.iter_mut().zip(collations) {
                if slot.is_none() {
                    *slot = collation;
                }
            }
        }
        return out;
    }
    let tables: Vec<(&TableDef, Option<&str>)> = match &plan.source {
        SelectSource::Table(table) => vec![(table.as_ref(), None)],
        SelectSource::Tables(bound) => bound
            .iter()
            .map(|b| (b.table.as_ref(), b.alias.as_deref()))
            .collect(),
        SelectSource::Joined(join) => std::iter::once(&join.base)
            .chain(join.joins.iter().map(|step| &step.right))
            .map(|b| (b.table.as_ref(), b.alias.as_deref()))
            .collect(),
        _ => Vec::new(),
    };
    let lookup = |expr: &Expr| -> Option<Collation> {
        if let Some(explicit) = collation_from_expr(expr) {
            return Some(explicit);
        }
        let (qualifier, name) = column_reference(expr)?;
        let single = tables.len() == 1;
        tables
            .iter()
            .find(|(table, alias)| {
                (single || qualifier.is_none_or(|q| names_table(table, *alias, q)))
                    && has_column(table, name)
            })
            .and_then(|(table, _)| declared_collation(table, name))
    };
    let mut out = Vec::with_capacity(plan.projection.len());
    for item in &plan.projection {
        match item {
            SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } => {
                out.push(lookup(expr));
            }
            SelectItem::Wildcard(_) => {
                for (table, _) in &tables {
                    out.extend(
                        table
                            .columns
                            .iter()
                            .map(|column| column.collation.as_deref().and_then(Collation::parse)),
                    );
                }
            }
            SelectItem::QualifiedWildcard(name, _) => {
                let qualifier = name.to_string();
                let qualifier = qualifier.trim_matches('"');
                for (table, alias) in &tables {
                    if names_table(table, *alias, qualifier) {
                        out.extend(
                            table.columns.iter().map(|column| {
                                column.collation.as_deref().and_then(Collation::parse)
                            }),
                        );
                    }
                }
            }
        }
    }
    out
}

/// `row` with each value folded by its column's collation, for DISTINCT and
/// set-operation keys.
pub(crate) fn collation_keyed_row(
    row: &[SqlValue],
    collations: &[Option<Collation>],
) -> Vec<SqlValue> {
    row.iter()
        .enumerate()
        .map(|(idx, value)| {
            collation_key(value.clone(), collations.get(idx).and_then(Option::as_ref))
        })
        .collect()
}
