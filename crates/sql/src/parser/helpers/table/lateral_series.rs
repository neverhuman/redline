//! `CROSS JOIN LATERAL generate_series(start, outer_column)`.
//!
//! The series is expanded once per already-materialized outer row and
//! paired back with an equijoin, so `t.x` and the series alias keep their
//! names for the SELECT list and ORDER BY.

use std::sync::Arc;

use redlinedb_kernel::catalog::SchemaSnapshot;
use sqlparser::ast::{
    BinaryOperator, Expr, FunctionArg, FunctionArgExpr, Ident, JoinConstraint, JoinOperator,
    TableFactor, TableWithJoins, Value,
};

use crate::error::{Error, Result};
use crate::statement::{BoundTable, JoinKind, JoinSource, JoinStep, ParamLayout, SelectSource};
use crate::value::SqlValue;

use super::select::bind_select_table_factor;

pub(crate) fn try_bind(
    conn: &crate::connection::Connection,
    schema: &SchemaSnapshot,
    from: &[TableWithJoins],
    params: &mut ParamLayout,
) -> Result<Option<SelectSource>> {
    if from.len() != 1 || from[0].joins.len() != 1 {
        return Ok(None);
    }
    let join = &from[0].joins[0];
    if !is_cross(&join.join_operator) {
        return Ok(None);
    }
    let TableFactor::Function {
        lateral: true,
        name,
        args,
        alias,
    } = &join.relation
    else {
        return Ok(None);
    };
    let func = name.to_string();
    if !func.eq_ignore_ascii_case("generate_series") {
        return Ok(None);
    }
    let series_alias = alias
        .as_ref()
        .map(|alias| alias.name.value.clone())
        .unwrap_or_else(|| "generate_series".to_owned());
    let series_column = alias
        .as_ref()
        .and_then(|alias| alias.columns.first())
        .map(|column| column.name.value.clone())
        .unwrap_or_else(|| series_alias.clone());

    let left = bind_select_table_factor(schema, from[0].relation.clone())?;
    let left_rows =
        crate::exec::cte::rows_for_relation(left.table.relation_id).ok_or_else(|| {
            Error::UnsupportedSql(
                "LATERAL generate_series needs a materialized outer input".to_owned(),
            )
        })?;
    let left_names: Vec<String> = left
        .table
        .columns
        .iter()
        .map(|column| column.name.to_string())
        .collect();
    let left_alias = left
        .alias
        .as_deref()
        .unwrap_or(left.table.name.as_ref())
        .to_owned();

    let mut paired_left = Vec::new();
    let mut paired_right = Vec::new();
    let mut key = 0i64;
    for row in left_rows.iter() {
        let start = arg_int(args.first(), &left_alias, &left_names, row)?;
        let stop = arg_int(args.get(1), &left_alias, &left_names, row)?;
        let step = match args.get(2) {
            Some(arg) => arg_int(Some(arg), &left_alias, &left_names, row)?,
            None => 1,
        };
        if step == 0 {
            return Err(Error::UnsupportedSql(
                "generate_series step must not be zero".to_owned(),
            ));
        }
        for value in series_values(start, stop, step)? {
            let mut left_row = row.clone();
            left_row.push(SqlValue::Integer(key));
            paired_left.push(left_row);
            paired_right.push(vec![SqlValue::Integer(value), SqlValue::Integer(key)]);
            key += 1;
        }
    }

    let key_name = "__rldb_lat_k";
    let mut left_columns = left_names;
    left_columns.push(key_name.to_owned());
    let left_def = crate::exec::cte::build_cte_def_from_rows(
        &format!("{left_alias}__lat"),
        left_columns,
        paired_left,
    );
    let right_def = crate::exec::cte::build_cte_def_from_rows(
        &format!("{series_alias}__lat"),
        vec![series_column, key_name.to_owned()],
        paired_right,
    );
    let left_table = BoundTable {
        table: left_def.table_def.expect("left lateral table"),
        alias: Some(Arc::from(left_alias.as_str())),
        index_hint: None,
    };
    let right_table = BoundTable {
        table: right_def.table_def.expect("right lateral table"),
        alias: Some(Arc::from(series_alias.as_str())),
        index_hint: None,
    };
    let selection = crate::parser::select::normalize_expr(
        Expr::BinaryOp {
            left: Box::new(Expr::CompoundIdentifier(vec![
                Ident::new(left_alias),
                Ident::new(key_name),
            ])),
            op: BinaryOperator::Eq,
            right: Box::new(Expr::CompoundIdentifier(vec![
                Ident::new(series_alias),
                Ident::new(key_name),
            ])),
        },
        params,
    )?;
    let _ = conn;
    Ok(Some(SelectSource::Joined(JoinSource {
        base: left_table,
        joins: vec![JoinStep {
            right: right_table,
            kind: JoinKind::Inner,
            selection: Some(selection),
            hidden_right_columns: Arc::from([]),
        }],
    })))
}

fn is_cross(op: &JoinOperator) -> bool {
    matches!(
        op,
        JoinOperator::CrossJoin(_) | JoinOperator::Join(JoinConstraint::None)
    )
}

fn series_values(start: i64, stop: i64, step: i64) -> Result<Vec<i64>> {
    if (step > 0 && start > stop) || (step < 0 && start < stop) {
        return Ok(Vec::new());
    }
    let count = ((i128::from(stop) - i128::from(start)) / i128::from(step) + 1) as usize;
    if count > 100_000 {
        return Err(Error::UnsupportedSql(
            "generate_series exceeds query work memory".to_owned(),
        ));
    }
    let mut values = Vec::with_capacity(count);
    let mut value = start;
    for index in 0..count {
        values.push(value);
        if index + 1 < count {
            value = value.checked_add(step).ok_or(Error::DatatypeMismatch)?;
        }
    }
    Ok(values)
}

fn arg_int(
    arg: Option<&FunctionArg>,
    alias: &str,
    columns: &[String],
    row: &[SqlValue],
) -> Result<i64> {
    let Some(FunctionArg::Unnamed(FunctionArgExpr::Expr(expr))) = arg else {
        return Err(Error::UnsupportedSql(
            "generate_series argument must be an expression".to_owned(),
        ));
    };
    expr_int(expr, alias, columns, row)
}

fn expr_int(expr: &Expr, alias: &str, columns: &[String], row: &[SqlValue]) -> Result<i64> {
    match expr {
        Expr::Value(value) => value_int(&value.value),
        Expr::Nested(inner) => expr_int(inner, alias, columns, row),
        Expr::UnaryOp {
            op: sqlparser::ast::UnaryOperator::Minus,
            expr,
        } => Ok(-expr_int(expr, alias, columns, row)?),
        Expr::Identifier(ident) => column_int(ident.value.as_str(), None, alias, columns, row),
        Expr::CompoundIdentifier(parts) if parts.len() == 2 => column_int(
            parts[1].value.as_str(),
            Some(parts[0].value.as_str()),
            alias,
            columns,
            row,
        ),
        _ => Err(Error::UnsupportedSql(
            "LATERAL generate_series argument is not a literal or outer column".to_owned(),
        )),
    }
}

fn column_int(
    name: &str,
    qualifier: Option<&str>,
    alias: &str,
    columns: &[String],
    row: &[SqlValue],
) -> Result<i64> {
    if let Some(qualifier) = qualifier
        && !qualifier.eq_ignore_ascii_case(alias)
    {
        return Err(Error::UnsupportedSql(format!(
            "LATERAL generate_series cannot see {qualifier}.{name}"
        )));
    }
    let index = columns
        .iter()
        .position(|column| column.eq_ignore_ascii_case(name))
        .ok_or_else(|| Error::UnknownColumn(name.to_owned()))?;
    match row.get(index) {
        Some(SqlValue::Integer(value)) => Ok(*value),
        Some(SqlValue::Real(value)) if value.is_finite() => Ok(*value as i64),
        Some(SqlValue::Null) => Err(Error::DatatypeMismatch),
        _ => Err(Error::DatatypeMismatch),
    }
}

fn value_int(value: &Value) -> Result<i64> {
    match value {
        Value::Number(text, _) => text.parse::<i64>().map_err(|_| Error::DatatypeMismatch),
        _ => Err(Error::DatatypeMismatch),
    }
}
