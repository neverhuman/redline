use super::*;
use std::cell::RefCell;

use super::affinity::CmpAffinity;
use crate::collation::Collation;
use redlinedb_kernel::catalog::Affinity;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct SubqueryCacheKey {
    ast_addr: usize,
    schema_epoch: u64,
    stats_epoch: u64,
    optimizer_hash: u64,
}

thread_local! {
    static SUBQUERY_TEMPLATE_CACHE: RefCell<ahash::AHashMap<SubqueryCacheKey, PreparedTemplate>> =
        RefCell::new(ahash::AHashMap::new());
    static IN_SUBQUERY_ROW_CACHE: RefCell<ahash::AHashMap<SubqueryCacheKey, Vec<Vec<SqlValue>>>> =
        RefCell::new(ahash::AHashMap::new());
}

pub(crate) fn clear_subquery_template_cache() {
    SUBQUERY_TEMPLATE_CACHE.with(|cache| cache.borrow_mut().clear());
    IN_SUBQUERY_ROW_CACHE.with(|cache| cache.borrow_mut().clear());
}

pub(crate) fn truthy_opt(value: &SqlValue) -> Option<bool> {
    match value {
        SqlValue::Null => None,
        _ => Some(super::pg_bool_or_truthy(value)),
    }
}

pub(crate) trait CaseEvaluator {
    fn eval_case_expr(&mut self, expr: &Expr) -> Result<SqlValue>;

    /// The comparison affinity of `CASE operand WHEN value`: SQLite codes
    /// each WHEN as `operand = value`. Contexts that cannot resolve columns
    /// convert nothing.
    fn comparison_affinity(&self, _operand: &Expr, _value: &Expr) -> CmpAffinity {
        CmpAffinity::None
    }

    /// The collation `CASE operand WHEN condition` compares under (Q5-10).
    fn case_collation(
        &self,
        _operand: &Expr,
        _condition: &Expr,
    ) -> Option<crate::collation::Collation> {
        None
    }
}

pub(crate) fn eval_case<E>(
    operand: Option<&Expr>,
    conditions: &[sqlparser::ast::CaseWhen],
    else_result: Option<&Expr>,
    evaluator: &mut E,
) -> Result<SqlValue>
where
    E: CaseEvaluator,
{
    if let Some(operand_expr) = operand {
        let operand = evaluator.eval_case_expr(operand_expr)?;
        if matches!(operand, SqlValue::Null) {
            return match else_result {
                Some(expr) => evaluator.eval_case_expr(expr),
                None => Ok(SqlValue::Null),
            };
        }
        for when in conditions {
            let condition = evaluator.eval_case_expr(&when.condition)?;
            if matches!(condition, SqlValue::Null) {
                continue;
            }
            // Q5-10: after the comparison affinity, the WHEN compares under
            // its collation.
            let collation = evaluator.case_collation(operand_expr, &when.condition);
            let equal = if super::affinity::may_convert(&operand, &condition) {
                let affinity = evaluator.comparison_affinity(operand_expr, &when.condition);
                let (left, right) =
                    super::affinity::apply_pair(operand.clone(), condition, affinity);
                compare_with_collation(&left, &right, collation.as_ref()) == Ordering::Equal
            } else {
                compare_with_collation(&operand, &condition, collation.as_ref()) == Ordering::Equal
            };
            if equal {
                return evaluator.eval_case_expr(&when.result);
            }
        }
    } else {
        for when in conditions {
            let condition = evaluator.eval_case_expr(&when.condition)?;
            if !matches!(condition, SqlValue::Null) && super::pg_bool_or_truthy(&condition) {
                return evaluator.eval_case_expr(&when.result);
            }
        }
    }
    match else_result {
        Some(expr) => evaluator.eval_case_expr(expr),
        None => Ok(SqlValue::Null),
    }
}

pub(crate) fn eval_subquery_value(
    subquery: &sqlparser::ast::Query,
    row: &RowContext<'_>,
    bindings: &[Option<SqlValue>],
) -> Result<SqlValue> {
    // SQLite scalar-subquery semantics
    // (https://sqlite.org/lang_expr.html#subqueries): a multi-row
    // subquery returns the value of the first row (in projection
    // order). A multi-column subquery is still rejected since the
    // expression context demands a single column.
    match evaluate_subquery_first_row(subquery, row, bindings)? {
        None => Ok(SqlValue::Null),
        Some(first) if first.is_empty() => Ok(SqlValue::Null),
        Some(first) if first.len() == 1 => Ok(first[0].clone()),
        Some(_) => Err(Error::UnsupportedSql(
            "scalar subquery must return exactly one column".to_owned(),
        )),
    }
}

fn bind_subquery(conn: &Connection, subquery: &sqlparser::ast::Query) -> Result<PreparedTemplate> {
    let key = subquery_cache_key(conn, subquery);
    if let Some(template) = SUBQUERY_TEMPLATE_CACHE.with(|cache| cache.borrow().get(&key).cloned())
    {
        return Ok(template);
    }
    let schema =
        current_tx_schema_snapshot(conn).unwrap_or_else(|| conn.engine().schema_snapshot());
    let template = crate::parser::bind_query(
        conn,
        schema,
        conn.schema_epoch(),
        "<subquery>",
        subquery.clone(),
    )?;
    SUBQUERY_TEMPLATE_CACHE.with(|cache| {
        cache.borrow_mut().insert(key, template.clone());
    });
    Ok(template)
}

fn subquery_cache_key(conn: &Connection, subquery: &sqlparser::ast::Query) -> SubqueryCacheKey {
    SubqueryCacheKey {
        ast_addr: subquery as *const sqlparser::ast::Query as usize,
        schema_epoch: conn.schema_epoch().0,
        stats_epoch: conn.stats_epoch().0,
        optimizer_hash: conn.optimizer_hash(),
    }
}

fn evaluate_subquery_first_row(
    subquery: &sqlparser::ast::Query,
    outer_row: &RowContext<'_>,
    bindings: &[Option<SqlValue>],
) -> Result<Option<Vec<SqlValue>>> {
    let Some(conn) = current_connection() else {
        return Err(Error::TransactionState(
            "subquery evaluation requires an active connection",
        ));
    };
    let template = bind_subquery(conn, subquery)?;
    let owned = outer_row.to_owned_row();
    crate::exec::with_outer_row(owned, || {
        crate::exec::materialize_first_prepared_row(conn, &template, bindings)
    })
}

pub(crate) fn evaluate_subquery_exists(
    subquery: &sqlparser::ast::Query,
    outer_row: &RowContext<'_>,
    bindings: &[Option<SqlValue>],
) -> Result<bool> {
    let Some(conn) = current_connection() else {
        return Err(Error::TransactionState(
            "subquery evaluation requires an active connection",
        ));
    };
    let template = bind_subquery(conn, subquery)?;
    let owned = outer_row.to_owned_row();
    crate::exec::with_outer_row(owned, || {
        crate::exec::prepared_select_has_row(conn, &template, bindings)
    })
}

fn row_values_for_expr(
    expr: &Expr,
    row: &RowContext<'_>,
    bindings: &[Option<SqlValue>],
) -> Result<Vec<SqlValue>> {
    match expr {
        Expr::Tuple(exprs) => exprs
            .iter()
            .map(|expr| eval_scalar(expr, row, bindings))
            .collect(),
        Expr::Nested(inner) => row_values_for_expr(inner, row, bindings),
        _ => Ok(vec![eval_scalar(expr, row, bindings)?]),
    }
}

/// Row-value equality under comparison affinity. `affinity(i)` gives the
/// affinity of component `i`; it is only asked when a component pair could
/// compare differently under some affinity, so callers compute it lazily.
fn row_eq(
    left: &[SqlValue],
    right: &[SqlValue],
    affinity: &mut dyn FnMut(usize) -> CmpAffinity,
    collations: &[Option<Collation>],
) -> Result<Option<bool>> {
    if left.len() != right.len() {
        return Err(Error::UnsupportedSql(format!(
            "row value arity mismatch: {} vs {}",
            left.len(),
            right.len()
        )));
    }
    for (i, (l, r)) in left.iter().zip(right.iter()).enumerate() {
        if matches!(l, SqlValue::Null) || matches!(r, SqlValue::Null) {
            return Ok(None);
        }
        let collation = collations.get(i).and_then(Option::as_ref);
        let ord = if super::affinity::may_convert(l, r) {
            let (l, r) = super::affinity::apply_pair(l.clone(), r.clone(), affinity(i));
            compare_with_collation(&l, &r, collation)
        } else {
            compare_with_collation(l, r, collation)
        };
        if ord != Ordering::Equal {
            return Ok(Some(false));
        }
    }
    Ok(Some(true))
}

/// The collations of the left operand of `IN`: one per row-value
/// component, each its explicit COLLATE or its column's (Q5-10).
fn in_lhs_collations(expr: &Expr, row: &RowContext<'_>) -> Vec<Option<Collation>> {
    match expr {
        Expr::Tuple(exprs) => exprs.iter().map(|expr| expr_collation(expr, row)).collect(),
        Expr::Nested(inner) => in_lhs_collations(inner, row),
        _ => vec![expr_collation(expr, row)],
    }
}

/// The affinities of the left operand of `IN`: one per row-value component.
fn in_lhs_affinities(expr: &Expr, row: &RowContext<'_>) -> Vec<Option<Affinity>> {
    match expr {
        Expr::Tuple(exprs) => exprs
            .iter()
            .map(|expr| super::affinity::expr_affinity(row, expr))
            .collect(),
        Expr::Nested(inner) => in_lhs_affinities(inner, row),
        _ => vec![super::affinity::expr_affinity(row, expr)],
    }
}

/// Lazily computed per-component affinities, filled on first use.
struct LazyAffinities<'a> {
    cached: Option<Vec<CmpAffinity>>,
    compute: Box<dyn FnMut() -> Vec<CmpAffinity> + 'a>,
}

impl<'a> LazyAffinities<'a> {
    fn new(compute: impl FnMut() -> Vec<CmpAffinity> + 'a) -> Self {
        Self {
            cached: None,
            compute: Box::new(compute),
        }
    }

    fn get(&mut self, index: usize) -> CmpAffinity {
        if self.cached.is_none() {
            self.cached = Some((self.compute)());
        }
        self.cached
            .as_ref()
            .and_then(|all| all.get(index).copied())
            .unwrap_or(CmpAffinity::None)
    }
}

/// The affinity of result column `index` of a scalar or IN subquery, from
/// its bound plan (`sqlite3ExprAffinity` of that result expression).
pub(crate) fn subquery_column_affinity(
    subquery: &sqlparser::ast::Query,
    index: usize,
) -> Option<Affinity> {
    let conn = current_connection()?;
    let template = bind_subquery(conn, subquery).ok()?;
    match &template.kind {
        crate::statement::PreparedKind::Select(plan) => {
            super::affinity::plan_column_affinity(plan, index)
        }
        _ => None,
    }
}

pub(crate) fn in_list_result(
    expr: &Expr,
    list: &[Expr],
    negated: bool,
    row: &RowContext<'_>,
    bindings: &[Option<SqlValue>],
) -> Result<SqlValue> {
    let value = row_values_for_expr(expr, row, bindings)?;
    if value.iter().any(|v| matches!(v, SqlValue::Null)) {
        return Ok(SqlValue::Null);
    }
    let mut found = false;
    let mut saw_null = false;
    // SQLite compares `x IN (list)` with the affinity of `x` alone.
    let mut affinities = LazyAffinities::new(|| {
        in_lhs_affinities(expr, row)
            .into_iter()
            .map(CmpAffinity::of_optional)
            .collect()
    });
    let mut lhs_collations: Option<Vec<Option<Collation>>> = None;
    for item in list {
        let candidate = row_values_for_expr(item, row, bindings)?;
        // Q5-10: a single value compares as `expr = item` would; a row value
        // compares each component under its left operand's collation.
        let item_collations;
        let collations = if value.len() == 1 {
            item_collations = [comparison_collation(expr, item, row)];
            &item_collations[..]
        } else {
            lhs_collations.get_or_insert_with(|| in_lhs_collations(expr, row))
        };
        match row_eq(&value, &candidate, &mut |i| affinities.get(i), collations)? {
            Some(true) => {
                found = true;
                break;
            }
            Some(false) => {}
            None => saw_null = true,
        }
    }
    finish_in_result(found, saw_null, negated)
}

pub(crate) fn in_subquery_result(
    expr: &Expr,
    subquery: &sqlparser::ast::Query,
    negated: bool,
    row: &RowContext<'_>,
    bindings: &[Option<SqlValue>],
) -> Result<SqlValue> {
    let value = row_values_for_expr(expr, row, bindings)?;
    if value.iter().any(|v| matches!(v, SqlValue::Null)) {
        return Ok(SqlValue::Null);
    }
    let Some(conn) = current_connection() else {
        return Err(Error::TransactionState(
            "subquery evaluation requires an active connection",
        ));
    };
    let template = bind_subquery(conn, subquery)?;
    if template.output_columns.len() != value.len() {
        return Err(Error::UnsupportedSql(
            "IN subquery must return the same number of columns as the row value".to_owned(),
        ));
    }
    let cache_key = subquery_cache_key(conn, subquery);
    // `x IN (SELECT y ...)` compares with the affinity of `x` against `y`.
    let mut affinities = LazyAffinities::new(|| {
        let lhs = in_lhs_affinities(expr, row);
        lhs.into_iter()
            .enumerate()
            .map(|(i, left)| CmpAffinity::between(left, subquery_column_affinity(subquery, i)))
            .collect()
    });
    let mut affinity = |i| affinities.get(i);
    // Q5-10: `x IN (SELECT ...)` compares under the collation of `x`.
    let collations = in_lhs_collations(expr, row);
    if in_subquery_is_cacheable(subquery) {
        if let Some(result) = IN_SUBQUERY_ROW_CACHE.with(|cache| {
            let cache = cache.borrow();
            cache.get(&cache_key).map(|rows| {
                finish_in_rows(
                    &value,
                    rows.iter().map(Vec::as_slice),
                    negated,
                    &mut affinity,
                    &collations,
                )
            })
        }) {
            return result;
        }

        let owned = row.to_owned_row();
        let (rows, used_correlated_lookup) = crate::exec::with_outer_row(owned, || {
            crate::exec::with_correlated_lookup_tracking(|| {
                materialize_prepared_rows(conn, &template, bindings)
            })
        });
        let rows = rows?;
        let result = finish_in_rows(
            &value,
            rows.iter().map(Vec::as_slice),
            negated,
            &mut affinity,
            &collations,
        )?;
        if !used_correlated_lookup {
            IN_SUBQUERY_ROW_CACHE.with(|cache| {
                cache.borrow_mut().insert(cache_key, rows);
            });
        }
        return Ok(result);
    }

    let owned = row.to_owned_row();
    let rows = crate::exec::with_outer_row(owned, || {
        materialize_prepared_rows(conn, &template, bindings)
    })?;
    finish_in_rows(
        &value,
        rows.iter().map(Vec::as_slice),
        negated,
        &mut affinity,
        &collations,
    )
}

fn finish_in_rows<'a, I>(
    value: &[SqlValue],
    rows: I,
    negated: bool,
    affinity: &mut dyn FnMut(usize) -> CmpAffinity,
    collations: &[Option<Collation>],
) -> Result<SqlValue>
where
    I: IntoIterator<Item = &'a [SqlValue]>,
{
    let mut found = false;
    let mut saw_null = false;
    for row in rows {
        match row_eq(value, row, affinity, collations)? {
            Some(true) => {
                found = true;
                break;
            }
            Some(false) => {}
            None => saw_null = true,
        }
    }
    finish_in_result(found, saw_null, negated)
}

fn in_subquery_is_cacheable(subquery: &sqlparser::ast::Query) -> bool {
    // A12: drop the per-row `to_ascii_lowercase()` allocation. `Query`'s
    // Display still allocates the rendered string (sqlparser API), but
    // we don't need to clone-and-downcase it to do case-insensitive
    // substring checks — the byte-scan helper handles that allocation-
    // free for every marker we're looking for. The list of volatile/
    // session-bound function names is closed and ASCII-only, so a
    // simple `eq_ignore_ascii_case` window check is sufficient.
    const VOLATILE_MARKERS: &[&[u8]] = &[
        b"random(",
        b"randomblob(",
        b"last_insert_rowid",
        b"changes(",
        b"total_changes(",
        b"current_date",
        b"current_time",
        b"current_timestamp",
        b"date(",
        b"time(",
        b"datetime(",
        b"julianday(",
        b"unixepoch(",
        b"strftime(",
    ];
    let rendered = subquery.to_string();
    if rendered.contains('.') {
        return false;
    }
    let bytes = rendered.as_bytes();
    for marker in VOLATILE_MARKERS {
        if contains_token_ci(bytes, marker) {
            return false;
        }
    }
    true
}

/// A12 helper: allocation-free case-insensitive substring scan. Same shape
/// as A7/A8/A9 byte-scans elsewhere.
#[inline]
fn contains_token_ci(haystack: &[u8], needle: &[u8]) -> bool {
    if haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle.iter())
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    })
}

fn finish_in_result(found: bool, saw_null: bool, negated: bool) -> Result<SqlValue> {
    let base_in: Option<bool> = if found {
        Some(true)
    } else if saw_null {
        None
    } else {
        Some(false)
    };
    Ok(match (base_in, negated) {
        (Some(b), false) => SqlValue::Integer(if b { 1 } else { 0 }),
        (Some(b), true) => SqlValue::Integer(if !b { 1 } else { 0 }),
        (None, _) => SqlValue::Null,
    })
}
