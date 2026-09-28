//! NEW-01: ORDER BY terms that name a result column by position.
//!
//! The binder (`parser/order_by.rs`) leaves `ORDER BY 2` as the bare
//! integer literal `2`, under a `COLLATE` when one applies, and wraps every
//! constant that folded to a number in parentheses. So a bare integer
//! literal in a bound ORDER BY is always a position, and every executor
//! that sorts reads its key from the projected result row instead of
//! evaluating the literal.

use std::sync::Arc;

use sqlparser::ast::{Expr, OrderByExpr, Value, ValueWithSpan};

use crate::error::Result;
use crate::value::SqlValue;

use super::expr::{RowContext, collation_from_expr, eval_scalar};

/// The 0-based result column an ORDER BY term names by position.
pub(crate) fn order_position(expr: &Expr) -> Option<usize> {
    let literal = match expr {
        Expr::Collate { expr, .. } => expr.as_ref(),
        other => other,
    };
    match literal {
        Expr::Value(ValueWithSpan {
            value: Value::Number(text, _),
            ..
        }) => text.parse::<usize>().ok()?.checked_sub(1),
        _ => None,
    }
}

/// True when some term of `order_by` is a position.
pub(crate) fn has_position(order_by: &[OrderByExpr]) -> bool {
    order_by
        .iter()
        .any(|order| order_position(&order.expr).is_some())
}

/// The raw sort value of `order` for one row: the projected result column
/// for a position, otherwise the term evaluated on the source row.
pub(crate) fn order_value(
    order: &OrderByExpr,
    row: &RowContext<'_>,
    projected: &[SqlValue],
    bindings: &[Option<SqlValue>],
) -> Result<SqlValue> {
    match order_position(&order.expr) {
        Some(column) => Ok(projected.get(column).cloned().unwrap_or(SqlValue::Null)),
        None => eval_scalar(&order.expr, row, bindings),
    }
}

/// Fold a TEXT sort value by the term's explicit collation, so plain value
/// comparison sorts it the way the collation compares.
pub(crate) fn collate_sort_value(order: &OrderByExpr, value: SqlValue) -> SqlValue {
    let Some(collation) = collation_from_expr(&order.expr) else {
        return value;
    };
    match value {
        SqlValue::Text(text) => SqlValue::Text(Arc::from(collation.sort_text(&text))),
        value => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlparser::dialect::SQLiteDialect;
    use sqlparser::parser::Parser;

    fn parse_expr(text: &str) -> Expr {
        Parser::new(&SQLiteDialect {})
            .try_with_sql(text)
            .expect("parser init")
            .parse_expr()
            .expect("parse expr")
    }

    #[test]
    fn bare_literals_are_positions_and_parenthesized_ones_are_not() {
        assert_eq!(order_position(&parse_expr("1")), Some(0));
        assert_eq!(order_position(&parse_expr("3 COLLATE NOCASE")), Some(2));
        assert_eq!(order_position(&parse_expr("(1)")), None);
        assert_eq!(order_position(&parse_expr("(1) COLLATE NOCASE")), None);
        assert_eq!(order_position(&parse_expr("0")), None);
        assert_eq!(order_position(&parse_expr("1.5")), None);
        assert_eq!(order_position(&parse_expr("x")), None);
    }
}
