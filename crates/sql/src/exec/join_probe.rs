//! Index lookup for an inner equijoin, instead of pairing every left row
//! with every right row.

use std::sync::Arc;

use sqlparser::ast::{BinaryOperator, Expr, Ident, Value, ValueWithSpan};

use super::load_table_row_by_rowid;
use crate::exec::expr::eval_scalar;
use crate::exec::expr::scalar::row::{JoinedRow, RowContext, TableRow};
use crate::exec::index_access::{self, execute_index_probe, try_match_index_access};
use crate::statement::JoinStep;
use crate::value::SqlValue;

use super::*;

pub(crate) fn can_probe(step: &JoinStep) -> bool {
    split_equijoin(step).is_some()
}

pub(crate) fn probe_inner_equijoin(
    engine: &Engine,
    tx: &mut Txn,
    prefix: &[JoinedRow],
    step: &JoinStep,
    bindings: &[Option<SqlValue>],
) -> Result<Option<Vec<TableRow>>> {
    let Some((ordinal, probe_expr)) = split_equijoin(step) else {
        return Ok(None);
    };
    let prefix_ctx = RowContext::Joined(prefix);
    let value = match eval_scalar(probe_expr, &prefix_ctx, bindings) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    if matches!(value, SqlValue::Null) {
        return Ok(Some(Vec::new()));
    }
    // `left.x = right.y` compares with the affinity of both operands
    // (`sqlite3IndexAffinityOk` decides whether the index on `right.y` can
    // answer). The index path converts the probe by `y`'s affinity; where
    // the comparison converts nothing, that conversion must leave this
    // probe as it is. Otherwise the join compares row by row.
    let Some(column) = step.right.table.columns.get(ordinal) else {
        return Ok(None);
    };
    let probe_affinity = crate::exec::expr::affinity::expr_affinity(&prefix_ctx, probe_expr);
    if !crate::exec::expr::affinity::index_usable(column.affinity, probe_affinity) {
        return Ok(None);
    }
    if crate::exec::expr::affinity::CmpAffinity::between(Some(column.affinity), probe_affinity)
        == crate::exec::expr::affinity::CmpAffinity::None
        && !crate::exec::expr::affinity::probe_unchanged_by_index(column.affinity, &value)
    {
        return Ok(None);
    }
    let Some(predicate) = equality_expr(&step.right.table, ordinal, &value) else {
        return Ok(None);
    };
    let selection = Some(predicate);
    let Some(matched) = try_match_index_access(engine, &step.right.table, &selection, bindings)
    else {
        return Ok(None);
    };
    if index_access::open_handle(engine, tx, &matched.index).is_none() {
        return Ok(None);
    }
    let rowids = execute_index_probe(
        engine,
        tx,
        &step.right.table,
        &matched.index,
        &matched.probe,
    )?;
    let mut rows = Vec::with_capacity(rowids.len());
    for rowid in rowids {
        if let Some(row) = load_table_row_by_rowid(engine, tx, &step.right.table, rowid)? {
            rows.push(row);
        }
    }
    Ok(Some(rows))
}

fn split_equijoin(step: &JoinStep) -> Option<(usize, &Expr)> {
    let expr = strip(step.selection.as_ref()?);
    let Expr::BinaryOp { left, op, right } = expr else {
        return None;
    };
    if !matches!(op, BinaryOperator::Eq) {
        return None;
    }
    let alias = step.right.alias.as_deref();
    let table = step.right.table.as_ref();
    if let Some(ordinal) = right_column(left, table, alias) {
        if right_column(right, table, alias).is_none() {
            return Some((ordinal, right));
        }
    }
    if let Some(ordinal) = right_column(right, table, alias) {
        if right_column(left, table, alias).is_none() {
            return Some((ordinal, left));
        }
    }
    None
}

fn right_column(expr: &Expr, table: &TableDef, alias: Option<&str>) -> Option<usize> {
    match strip(expr) {
        Expr::CompoundIdentifier(parts) if parts.len() >= 2 => {
            let qual = parts[parts.len() - 2].value.as_str();
            let ok = qual.eq_ignore_ascii_case(table.name.as_ref())
                || alias.is_some_and(|alias| qual.eq_ignore_ascii_case(alias));
            if !ok {
                return None;
            }
            let col = parts.last()?.value.as_str();
            column_ordinal(col, table).filter(|ordinal| indexed(*ordinal, table))
        }
        _ => None,
    }
}

fn column_ordinal(name: &str, table: &TableDef) -> Option<usize> {
    table
        .columns
        .iter()
        .position(|column| column.folded.as_ref().eq_ignore_ascii_case(name))
}

fn indexed(ordinal: usize, table: &TableDef) -> bool {
    table.indexes.iter().any(|index| {
        index
            .keys
            .first()
            .is_some_and(|key| key.ordinal as usize == ordinal)
    })
}

fn equality_expr(table: &Arc<TableDef>, ordinal: usize, value: &SqlValue) -> Option<Expr> {
    let column = table.columns.get(ordinal)?;
    let literal = match value {
        SqlValue::Integer(n) => Value::Number(n.to_string(), false),
        SqlValue::Text(text) => Value::SingleQuotedString(text.to_string()),
        _ => return None,
    };
    Some(Expr::BinaryOp {
        left: Box::new(Expr::Identifier(Ident::new(column.name.to_string()))),
        op: BinaryOperator::Eq,
        right: Box::new(Expr::Value(ValueWithSpan {
            value: literal,
            span: sqlparser::tokenizer::Span::empty(),
        })),
    })
}

pub(crate) fn prefilter_base(
    rows: &mut Vec<Vec<JoinedRow>>,
    base: &crate::statement::BoundTable,
    selection: &Option<Expr>,
    bindings: &[Option<SqlValue>],
) -> Result<()> {
    let Some(expr) = selection else {
        return Ok(());
    };
    let parts = and_conjuncts(expr);
    let local: Vec<&Expr> = parts
        .into_iter()
        .filter(|part| mentions_only_base(part, base))
        .collect();
    if local.is_empty() {
        return Ok(());
    }
    let mut kept = Vec::with_capacity(rows.len());
    for prefix in rows.drain(..) {
        let mut ok = true;
        for pred in &local {
            let value = eval_scalar(pred, &RowContext::Joined(&prefix), bindings)?;
            if !crate::exec::expr::pg_bool_or_truthy(&value) {
                ok = false;
                break;
            }
        }
        if ok {
            kept.push(prefix);
        }
    }
    *rows = kept;
    Ok(())
}

fn and_conjuncts(expr: &Expr) -> Vec<&Expr> {
    let mut out = Vec::new();
    walk_and(expr, &mut out);
    out
}

fn walk_and<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    if let Expr::BinaryOp { left, op, right } = strip(expr)
        && matches!(op, BinaryOperator::And)
    {
        walk_and(left, out);
        walk_and(right, out);
    } else {
        out.push(strip(expr));
    }
}

fn mentions_only_base(expr: &Expr, base: &crate::statement::BoundTable) -> bool {
    match strip(expr) {
        Expr::CompoundIdentifier(parts) => {
            let Some(qual) = parts.get(parts.len().saturating_sub(2)) else {
                return false;
            };
            qual_is_base(qual.value.as_str(), base)
        }
        Expr::Identifier(ident) => {
            column_ordinal(ident.value.as_str(), base.table.as_ref()).is_some()
        }
        Expr::BinaryOp { left, right, .. } => {
            mentions_only_base(left, base) && mentions_only_base(right, base)
        }
        Expr::UnaryOp { expr, .. } => mentions_only_base(expr, base),
        Expr::Nested(inner) => mentions_only_base(inner, base),
        Expr::Value(_) => true,
        _ => false,
    }
}

fn qual_is_base(qual: &str, base: &crate::statement::BoundTable) -> bool {
    qual.eq_ignore_ascii_case(base.table.name.as_ref())
        || base
            .alias
            .as_deref()
            .is_some_and(|alias| qual.eq_ignore_ascii_case(alias))
}

fn strip(expr: &Expr) -> &Expr {
    let mut current = expr;
    while let Expr::Nested(inner) = current {
        current = inner;
    }
    current
}

#[cfg(test)]
#[path = "join_probe_tests.rs"]
mod tests;
