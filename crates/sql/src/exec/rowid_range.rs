//! A seek on the rowid for WHERE conjuncts that bound it.
//!
//! `WHERE id BETWEEN ? AND ?`, `id >= ? AND id < ?` or `id = ? AND …` on a
//! rowid alias had no access path when no index applied, so the query read
//! every row of the table. The conjuncts that compare the rowid with a
//! constant bound an interval, and only the rows whose rowid is in it are
//! read.
//!
//! The interval may be wider than the WHERE clause: a REAL bound is widened
//! to the integers around it, strict or not, and saturates at the ends of
//! the i64 range. Every row read still passes the whole WHERE clause, so a
//! wider interval only costs reads. It is never narrower: a conjunct that
//! is not a comparison of the rowid with a number leaves the interval as it
//! is.

use std::sync::Arc;

use redlinedb_kernel::catalog::TableDef;
use redlinedb_kernel::engine::Engine;
use redlinedb_kernel::format::RowId;
use sqlparser::ast::{BinaryOperator, Expr, UnaryOperator};

use crate::error::Result;
use crate::planner::helpers::names_rowid;
use crate::value::SqlValue;

use super::{RowContext, eval_scalar};

/// The rowids of `table` that the WHERE clause's rowid bounds admit, in the
/// order a table scan visits them, or `None` when it bounds no rowid.
pub(super) fn table_rowids(
    engine: &Engine,
    table: &Arc<TableDef>,
    selection: &Option<Expr>,
    bindings: &[Option<SqlValue>],
) -> Result<Option<Vec<RowId>>> {
    if crate::exec::cte::is_cte_table_def(table) {
        return Ok(None);
    }
    let Some((low, high)) = interval(table, selection, bindings)? else {
        return Ok(None);
    };
    if low > high {
        return Ok(Some(Vec::new()));
    }
    // Row ids are the i64 rowid's bits as a u64, and a scan visits them in
    // u64 order: the non-negative rowids ascending, then the negative ones.
    let rowid = |value: i64| RowId(value as u64);
    let mut rowids = Vec::new();
    if high >= 0 {
        rowids.extend(engine.relation_rowids_between(
            table.relation_id,
            rowid(low.max(0)),
            rowid(high),
        )?);
    }
    if low < 0 {
        rowids.extend(engine.relation_rowids_between(
            table.relation_id,
            rowid(low),
            rowid(high.min(-1)),
        )?);
    }
    Ok(Some(rowids))
}

/// Whether the WHERE clause bounds `table`'s rowid.
pub(in crate::exec) fn bounds_rowid(
    table: &TableDef,
    selection: &Option<Expr>,
    bindings: &[Option<SqlValue>],
) -> Result<bool> {
    Ok(!crate::exec::cte::is_cte_table_def(table)
        && interval(table, selection, bindings)?.is_some())
}

/// The inclusive interval `low..=high` holding every rowid the top-level
/// conjuncts of `selection` admit, or `None` when none of them bounds it.
fn interval(
    table: &TableDef,
    selection: &Option<Expr>,
    bindings: &[Option<SqlValue>],
) -> Result<Option<(i64, i64)>> {
    let Some(expr) = selection else {
        return Ok(None);
    };
    let mut conjuncts = Vec::new();
    flatten_and(expr, &mut conjuncts);
    let mut bounded = None;
    for conjunct in conjuncts {
        if let Some((low, high)) = conjunct_bounds(table, conjunct, bindings)? {
            let (old_low, old_high) = bounded.unwrap_or((i64::MIN, i64::MAX));
            bounded = Some((old_low.max(low), old_high.min(high)));
        }
    }
    Ok(bounded)
}

fn flatten_and<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    match expr {
        Expr::BinaryOp {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            flatten_and(left, out);
            flatten_and(right, out);
        }
        Expr::Nested(inner) => flatten_and(inner, out),
        other => out.push(other),
    }
}

/// The rowids one conjunct admits, if it compares the rowid with a number.
fn conjunct_bounds(
    table: &TableDef,
    conjunct: &Expr,
    bindings: &[Option<SqlValue>],
) -> Result<Option<(i64, i64)>> {
    match conjunct {
        Expr::Between {
            expr,
            negated: false,
            low,
            high,
        } if names_rowid(table, expr) => {
            let (Some(low), Some(high)) = (number(low, bindings)?, number(high, bindings)?) else {
                return Ok(None);
            };
            Ok(Some((low.floor(), high.ceil())))
        }
        Expr::BinaryOp { left, op, right } => {
            let (op, value) = if names_rowid(table, left) {
                (op.clone(), right)
            } else if names_rowid(table, right) {
                let Some(op) = flipped(op) else {
                    return Ok(None);
                };
                (op, left)
            } else {
                return Ok(None);
            };
            let Some(value) = number(value, bindings)? else {
                return Ok(None);
            };
            Ok(match (op, value) {
                (BinaryOperator::Eq, value) => Some((value.floor(), value.ceil())),
                // A strict bound on an integer excludes it exactly.
                (BinaryOperator::Gt, Number::Integer(value)) => {
                    Some((value.saturating_add(1), i64::MAX))
                }
                (BinaryOperator::Lt, Number::Integer(value)) => {
                    Some((i64::MIN, value.saturating_sub(1)))
                }
                (BinaryOperator::Gt | BinaryOperator::GtEq, value) => {
                    Some((value.floor(), i64::MAX))
                }
                (BinaryOperator::Lt | BinaryOperator::LtEq, value) => {
                    Some((i64::MIN, value.ceil()))
                }
                _ => None,
            })
        }
        _ => Ok(None),
    }
}

/// `op` with its operands swapped: `5 < id` is `id > 5`.
fn flipped(op: &BinaryOperator) -> Option<BinaryOperator> {
    Some(match op {
        BinaryOperator::Eq => BinaryOperator::Eq,
        BinaryOperator::Lt => BinaryOperator::Gt,
        BinaryOperator::LtEq => BinaryOperator::GtEq,
        BinaryOperator::Gt => BinaryOperator::Lt,
        BinaryOperator::GtEq => BinaryOperator::LtEq,
        _ => return None,
    })
}

/// A number the rowid is compared with.
#[derive(Clone, Copy)]
enum Number {
    Integer(i64),
    Real(f64),
}

impl Number {
    /// The greatest integer at or below the number (saturating).
    fn floor(self) -> i64 {
        match self {
            Number::Integer(value) => value,
            Number::Real(value) => value.floor() as i64,
        }
    }

    /// The least integer at or above the number (saturating).
    fn ceil(self) -> i64 {
        match self {
            Number::Integer(value) => value,
            Number::Real(value) => value.ceil() as i64,
        }
    }
}

/// The number a constant operand compares as against the rowid, whose
/// INTEGER affinity makes the comparison numeric. `None` for anything that
/// is not a constant, for a constant that does not evaluate, and for NULL,
/// a blob, text that is not a number and NaN: the WHERE clause decides
/// those rows itself.
fn number(expr: &Expr, bindings: &[Option<SqlValue>]) -> Result<Option<Number>> {
    if !is_constant(expr) {
        return Ok(None);
    }
    // A constant that fails to evaluate bounds nothing; the WHERE clause
    // then reports the error for a row it reads, as it did before.
    let Ok(value) = eval_scalar(expr, &RowContext::Empty, bindings) else {
        return Ok(None);
    };
    let value = match value {
        SqlValue::Text(text) => match crate::numeric::text_number::comparison_number(&text) {
            Some(value) => value,
            None => return Ok(None),
        },
        value => value,
    };
    Ok(match value {
        SqlValue::Integer(value) => Some(Number::Integer(value)),
        SqlValue::Real(value) if !value.is_nan() => Some(Number::Real(value)),
        _ => None,
    })
}

/// A literal or a bound parameter, possibly signed or parenthesized.
fn is_constant(expr: &Expr) -> bool {
    match expr {
        Expr::Value(_) => true,
        Expr::Nested(inner) => is_constant(inner),
        Expr::UnaryOp {
            op: UnaryOperator::Minus | UnaryOperator::Plus,
            expr,
        } => is_constant(expr),
        _ => false,
    }
}
