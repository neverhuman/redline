use super::super::agg_eval::eval_group_scalar_with_ctx;
use super::super::expr::eval_scalar;
use super::super::*;

pub(crate) fn sort_groups_by_order_by(
    projected: &mut Vec<Vec<SqlValue>>,
    groups: &[&[SqlRow]],
    projection: &[SelectItem],
    order_by: &[OrderByExpr],
    bindings: &[Option<SqlValue>],
) -> Result<()> {
    if projected.len() != groups.len() {
        projected.sort_by(|l, r| compare_rows(l, r));
        return Ok(());
    }
    let mut keys: Vec<Vec<SqlValue>> = Vec::with_capacity(groups.len());
    for (group, row) in groups.iter().zip(projected.iter()) {
        let mut row_keys = Vec::with_capacity(order_by.len());
        for order in order_by {
            // NEW-01: `ORDER BY 2` sorts by the group's second result
            // column. Either kind of key sorts under the term's collation
            // (`ORDER BY g COLLATE NOCASE` compared bytes here before).
            let key = match super::super::order_position::order_position(&order.expr) {
                Some(column) => row.get(column).cloned().unwrap_or(SqlValue::Null),
                None => eval_grouped_order_key(&order.expr, group, projection, bindings)?,
            };
            row_keys.push(super::super::order_position::collate_sort_value(order, key));
        }
        keys.push(row_keys);
    }
    let mut rows: Vec<(usize, Vec<SqlValue>)> =
        std::mem::take(projected).into_iter().enumerate().collect();
    rows.sort_by(|(a, _), (b, _)| {
        for (idx, order) in order_by.iter().enumerate() {
            let mut ord = compare_values(&keys[*a][idx], &keys[*b][idx]);
            if matches!(order.options.asc, Some(false)) {
                ord = ord.reverse();
            }
            if ord != Ordering::Equal {
                return ord;
            }
        }
        Ordering::Equal
    });
    projected.extend(rows.into_iter().map(|(_, row)| row));
    Ok(())
}

fn eval_grouped_order_key(
    expr: &Expr,
    group: &[SqlRow],
    projection: &[SelectItem],
    bindings: &[Option<SqlValue>],
) -> Result<SqlValue> {
    if let Expr::Identifier(ident) = expr {
        for item in projection {
            if let SelectItem::ExprWithAlias { alias, expr } = item
                && alias.value.eq_ignore_ascii_case(&ident.value)
            {
                return eval_grouped_expr(expr, group, bindings);
            }
        }
    }
    eval_grouped_expr(expr, group, bindings)
}

fn eval_grouped_expr(
    expr: &Expr,
    group: &[SqlRow],
    bindings: &[Option<SqlValue>],
) -> Result<SqlValue> {
    let first_context = group.first().map(|row| row.context());
    eval_group_scalar_with_ctx(expr, group, first_context.as_ref(), bindings)
}

/// Sort `projected` rows in-place by `order_by`. Order expressions are
/// resolved against the projected output columns (by alias, or by the
/// rendered name of an unaliased projection expression). This supports
/// `SELECT DISTINCT x ... ORDER BY x DESC` after dedup.
pub(crate) fn sort_projected_rows_by_order_by(
    projected: &mut [Vec<SqlValue>],
    projection: &[SelectItem],
    order_by: &[OrderByExpr],
    bindings: &[Option<SqlValue>],
) -> Result<()> {
    enum OrderResolution {
        Column(usize),
        Constant(SqlValue),
    }

    let mut recipes: Vec<(OrderResolution, bool, &OrderByExpr)> =
        Vec::with_capacity(order_by.len());
    for order in order_by {
        let desc = matches!(order.options.asc, Some(false));
        let resolved = resolve_order_against_projection(&order.expr, projection)?;
        let resolution = match resolved {
            Some(idx) => OrderResolution::Column(idx),
            None => {
                // NEW-01: a position (`ORDER BY 2`, `2 COLLATE NOCASE`).
                if let Some(column) = super::super::order_position::order_position(&order.expr)
                    && column < projection_output_arity(projection)
                {
                    OrderResolution::Column(column)
                } else {
                    let ctx = RowContext::Empty;
                    OrderResolution::Constant(eval_scalar(&order.expr, &ctx, bindings)?)
                }
            }
        };
        recipes.push((resolution, desc, order));
    }

    projected.sort_by(|a, b| {
        for (recipe, desc, order) in &recipes {
            let (lv, rv) = match recipe {
                OrderResolution::Column(idx) => {
                    let lv = a.get(*idx).cloned().unwrap_or(SqlValue::Null);
                    let rv = b.get(*idx).cloned().unwrap_or(SqlValue::Null);
                    // `ORDER BY 1 COLLATE NOCASE` sorts by the collation.
                    (
                        super::super::order_position::collate_sort_value(order, lv),
                        super::super::order_position::collate_sort_value(order, rv),
                    )
                }
                OrderResolution::Constant(value) => (value.clone(), value.clone()),
            };
            let mut ord = compare_values(&lv, &rv);
            if *desc {
                ord = ord.reverse();
            }
            if ord != Ordering::Equal {
                return ord;
            }
        }
        Ordering::Equal
    });
    Ok(())
}

/// If `expr` is a bare identifier matching the alias of one of `projection`'s
/// items (or the rendered name of an unaliased simple projection), return
/// the corresponding output-column index.
fn resolve_order_against_projection(
    expr: &Expr,
    projection: &[SelectItem],
) -> Result<Option<usize>> {
    let target = match expr {
        Expr::Identifier(ident) => ident.value.as_str(),
        Expr::CompoundIdentifier(parts) if parts.len() == 1 => parts[0].value.as_str(),
        _ => return Ok(None),
    };
    let mut idx = 0usize;
    for item in projection {
        match item {
            SelectItem::ExprWithAlias { alias, .. } => {
                if alias.value.eq_ignore_ascii_case(target) {
                    return Ok(Some(idx));
                }
                idx += 1;
            }
            SelectItem::UnnamedExpr(expr) => {
                if matches_simple_identifier(expr, target) {
                    return Ok(Some(idx));
                }
                idx += 1;
            }
            SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(_, _) => {
                return Ok(None);
            }
        }
    }
    Ok(None)
}

fn projection_output_arity(projection: &[SelectItem]) -> usize {
    let mut arity = 0usize;
    for item in projection {
        match item {
            SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(_, _) => return usize::MAX,
            _ => arity += 1,
        }
    }
    arity
}

fn matches_simple_identifier(expr: &Expr, target: &str) -> bool {
    match expr {
        Expr::Identifier(ident) => ident.value.eq_ignore_ascii_case(target),
        Expr::CompoundIdentifier(parts) => parts
            .last()
            .is_some_and(|p| p.value.eq_ignore_ascii_case(target)),
        Expr::Nested(inner) => matches_simple_identifier(inner, target),
        _ => false,
    }
}

pub(crate) fn eval_group_key(
    group_by: &[Expr],
    row: &SqlRow,
    bindings: &[Option<SqlValue>],
) -> Result<Vec<SqlValue>> {
    let mut out = Vec::with_capacity(group_by.len());
    for expr in group_by {
        out.push(eval_scalar(expr, &row.context(), bindings)?);
    }
    Ok(out)
}
