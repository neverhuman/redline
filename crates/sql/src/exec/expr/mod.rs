//! Expression evaluation for the SQL executor.
//!
//! This module is the dispatcher between AST nodes and concrete evaluation
//! helpers. The bulk of the implementation is split across sibling files:
//!
//!   * `scalar`        — arithmetic / string / null helpers, vector and
//!     datetime helpers, row-context plumbing
//!   * `coerce`        — type coercion, comparison, binary-operator eval
//!   * `predicate`     — CASE / IN / EXISTS / scalar-subquery helpers
//!   * `json_dispatch` — function-call dispatcher (delegates JSON funcs
//!     to `crate::json::scalar`)
//!   * `window`        — window-function execution stubs (parsed-only)
//!
//! Submodules import shared symbols via `use super::*`. To keep that
//! ergonomic, this `mod.rs` glob-re-exports the items defined in the
//! sibling files (with `pub(super)` so the surface area outside `expr/`
//! does not change).

use super::*;

struct ScalarCaseEvaluator<'row, 'bindings> {
    row: &'row RowContext<'bindings>,
    bindings: &'row [Option<SqlValue>],
}

impl<'row, 'bindings> CaseEvaluator for ScalarCaseEvaluator<'row, 'bindings> {
    fn eval_case_expr(&mut self, expr: &Expr) -> Result<SqlValue> {
        eval_scalar(expr, self.row, self.bindings)
    }

    fn comparison_affinity(&self, operand: &Expr, value: &Expr) -> affinity::CmpAffinity {
        affinity::CmpAffinity::between(
            affinity::expr_affinity(self.row, operand),
            affinity::expr_affinity(self.row, value),
        )
    }

    fn case_collation(
        &self,
        operand: &Expr,
        condition: &Expr,
    ) -> Option<crate::collation::Collation> {
        comparison_collation(operand, condition, self.row)
    }
}

pub(crate) mod affinity;
pub(crate) mod coerce;
pub(crate) mod json_dispatch;
mod predicate;
// Phase 6 R1-C: Two-tier ScalarProgram VM (Tier 0). Parallel module —
// the AST→bytecode fallback wiring lands in a later round.
pub(crate) mod program;
pub(crate) mod scalar;
pub(crate) mod window;
pub(crate) mod window_eval;

// Glob-re-export every symbol from the sibling files so each sibling's
// `use super::*` (combined with the `use super::*` re-exporting `exec`'s
// own items above) sees the full pre-split surface area. This keeps the
// split a pure rename refactor — call sites in this `mod.rs` and inside
// the siblings reach unqualified helpers through these re-exports.
pub(crate) use coerce::*;
use json_dispatch::eval_function;
pub(crate) use predicate::*;
pub(crate) use scalar::*;

pub(crate) fn project_row(
    projection: &[SelectItem],
    row: &SqlRow,
    bindings: &[Option<SqlValue>],
) -> Result<Vec<SqlValue>> {
    if projection.is_empty() {
        return row.values();
    }

    // Phase 4.3: project_row is called per-row in every materialized
    // scan/agg/cte/window pipeline. Hinting capacity eliminates the
    // grow-by-doubling reallocation cascade for projections with >0
    // items (the dominant case).
    let mut out = Vec::with_capacity(projection.len());
    for item in projection {
        match item {
            SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(_, _) => {
                out.extend(row.values()?);
            }
            SelectItem::UnnamedExpr(expr) => out.push(eval_scalar(expr, &row.context(), bindings)?),
            SelectItem::ExprWithAlias { expr, .. } => {
                out.push(eval_scalar(expr, &row.context(), bindings)?)
            }
        }
    }
    Ok(out)
}

pub(crate) fn selection_passes(
    selection: &Option<Expr>,
    row: &SqlRow,
    bindings: &[Option<SqlValue>],
) -> Result<bool> {
    match selection {
        Some(expr) => {
            let value = eval_scalar(expr, &row.context(), bindings)?;
            Ok(pg_bool_or_truthy(&value))
        }
        None => Ok(true),
    }
}

/// SQLite truthiness + PostgreSQL `boolean` literal recognition. JSONB
/// operators in this crate return the textual tokens `"t"`/`"f"` to
/// match psql's unaligned output; treat those (and the spelled-out
/// `"true"`/`"false"`) as boolean values inside WHERE / CASE.
fn is_text_cast(data_type: &sqlparser::ast::DataType) -> bool {
    let name = data_type.to_string().to_ascii_lowercase();
    name == "text" || name == "varchar" || name.starts_with("char")
}

fn is_boolean_expr(expr: &Expr) -> bool {
    match expr {
        Expr::BinaryOp { op, .. } => matches!(
            op,
            BinaryOperator::Eq
                | BinaryOperator::NotEq
                | BinaryOperator::Lt
                | BinaryOperator::LtEq
                | BinaryOperator::Gt
                | BinaryOperator::GtEq
                | BinaryOperator::And
                | BinaryOperator::Or
        ),
        Expr::Nested(inner) | Expr::UnaryOp { expr: inner, .. } => is_boolean_expr(inner),
        _ => false,
    }
}

fn pg_bool_text(value: SqlValue) -> SqlValue {
    match value {
        SqlValue::Null => SqlValue::Null,
        SqlValue::Integer(0) => SqlValue::Text(Arc::from("false")),
        SqlValue::Integer(_) => SqlValue::Text(Arc::from("true")),
        SqlValue::Text(text)
            if text.eq_ignore_ascii_case("f") || text.eq_ignore_ascii_case("false") =>
        {
            SqlValue::Text(Arc::from("false"))
        }
        SqlValue::Text(_) => SqlValue::Text(Arc::from("true")),
        other => other,
    }
}

pub(crate) fn pg_bool_or_truthy(value: &SqlValue) -> bool {
    if let SqlValue::Text(s) = value {
        let trimmed = s.as_ref().trim();
        if trimmed.eq_ignore_ascii_case("t") || trimmed.eq_ignore_ascii_case("true") {
            return true;
        }
        if trimmed.eq_ignore_ascii_case("f") || trimmed.eq_ignore_ascii_case("false") {
            return false;
        }
    }
    is_truthy(value)
}

pub(crate) fn compare_rows(left: &[SqlValue], right: &[SqlValue]) -> Ordering {
    for (l, r) in left.iter().zip(right.iter()) {
        let ord = compare_values(l, r);
        if ord != Ordering::Equal {
            return ord;
        }
    }
    left.len().cmp(&right.len())
}

pub(crate) fn eval_scalar(
    expr: &Expr,
    row: &RowContext<'_>,
    bindings: &[Option<SqlValue>],
) -> Result<SqlValue> {
    // Phase 6 R2-A: opt-in ScalarProgram VM dispatch.
    //
    // When the process-wide toggle is OFF (default), `try_eval_scalar_via_vm`
    // short-circuits to `None` with zero overhead and the AST walker
    // below runs unchanged. When the toggle is ON, the helper attempts
    // to compile + evaluate via the Tier-0 bytecode VM. `None` indicates
    // an unsupported shape — we fall through and let the AST walker
    // handle it (the VM compile-failure counter is bumped from inside
    // the helper so the corpus diff can audit silent fall-backs).
    if let Some(result) = try_eval_scalar_via_vm(expr, row, bindings) {
        return result;
    }
    if let Expr::Value(v) = expr {
        if let Some(name) = crate::parser::bind::as_bind_name(&v.value) {
            return resolve_binding(name, bindings);
        }
    }
    Ok(match expr {
        Expr::Value(v) => match &v.value {
            Value::Null => SqlValue::Null,
            Value::Boolean(v) => SqlValue::Integer(if *v { 1 } else { 0 }),
            Value::Number(n, _) => parse_number(n)?,
            Value::SingleQuotedString(s)
            | Value::DoubleQuotedString(s)
            | Value::EscapedStringLiteral(s)
            | Value::TripleSingleQuotedString(s)
            | Value::TripleDoubleQuotedString(s)
            | Value::UnicodeStringLiteral(s)
            | Value::SingleQuotedRawStringLiteral(s)
            | Value::DoubleQuotedRawStringLiteral(s)
            | Value::TripleSingleQuotedRawStringLiteral(s)
            | Value::TripleDoubleQuotedRawStringLiteral(s) => SqlValue::Text(Arc::from(s.as_str())),
            Value::SingleQuotedByteStringLiteral(s)
            | Value::DoubleQuotedByteStringLiteral(s)
            | Value::TripleSingleQuotedByteStringLiteral(s)
            | Value::TripleDoubleQuotedByteStringLiteral(s) => {
                SqlValue::Blob(Arc::from(s.as_bytes()))
            }
            Value::HexStringLiteral(s) => SqlValue::Blob(hex_string_to_bytes(s)?),
            Value::DollarQuotedString(s) => SqlValue::Text(Arc::from(s.value.as_str())),
            other => {
                return Err(Error::UnsupportedSql(format!(
                    "unsupported SQL literal: {other:?}"
                )));
            }
        },
        Expr::Identifier(ident) => lookup_column(row, &ident.value)?,
        Expr::CompoundIdentifier(parts) => match parts.as_slice() {
            [ident] => lookup_column(row, &ident.value)?,
            [qualifier, ident] => lookup_qualified_column(row, &qualifier.value, &ident.value)?,
            // 3-part: `schema.table.column`. We collapse to the
            // `table.column` form: SQLite-parity callers reference an
            // attached database via the table alias, and the row binder
            // already keyed cross-DB tables under the unqualified
            // table name, so the schema qualifier is non-load-bearing
            // at this surface. The case is rare enough that we don't
            // distinguish `main.foo.col` from `aux.foo.col` here.
            [_schema, table, ident] => lookup_qualified_column(row, &table.value, &ident.value)?,
            _ => {
                return Err(Error::UnsupportedSql(format!(
                    "unsupported identifier: {parts:?}"
                )));
            }
        },
        Expr::Nested(expr) => eval_scalar(expr, row, bindings)?,
        Expr::Substring {
            expr,
            substring_from,
            substring_for,
            ..
        } => {
            let mut values = Vec::with_capacity(3);
            values.push(eval_scalar(expr, row, bindings)?);
            match substring_from {
                Some(from) => values.push(eval_scalar(from, row, bindings)?),
                None => values.push(SqlValue::Null),
            }
            match substring_for {
                Some(for_expr) => values.push(eval_scalar(for_expr, row, bindings)?),
                None => {}
            }
            sqlite_substr_function(&values)?
        }
        Expr::Trim {
            expr,
            trim_where,
            trim_what,
            trim_characters,
        } => {
            let value = eval_scalar(expr, row, bindings)?;
            let chars = if let Some(what) = trim_what {
                Some(eval_scalar(what, row, bindings)?)
            } else if let Some(chars) = trim_characters {
                let mut out = String::new();
                for ch in chars {
                    let ch_value = eval_scalar(ch, row, bindings)?;
                    if matches!(ch_value, SqlValue::Null) {
                        return Ok(SqlValue::Null);
                    }
                    out.push_str(&value_to_string(&ch_value));
                }
                Some(SqlValue::Text(Arc::from(out)))
            } else {
                None
            };
            let chars_ref = chars.as_ref();
            match trim_where {
                Some(sqlparser::ast::TrimWhereField::Leading) => {
                    sqlite_ltrim_function(&value, chars_ref)?
                }
                Some(sqlparser::ast::TrimWhereField::Trailing) => {
                    sqlite_rtrim_function(&value, chars_ref)?
                }
                _ => sqlite_trim_function(&value, chars_ref)?,
            }
        }
        Expr::UnaryOp { op, expr } => {
            let value = eval_scalar(expr, row, bindings)?;
            match op {
                UnaryOperator::Not => match truthy_opt(&value) {
                    Some(v) => SqlValue::Integer(if !v { 1 } else { 0 }),
                    None => SqlValue::Null,
                },
                UnaryOperator::Minus => negate(value)?,
                UnaryOperator::Plus => value,
                _ => {
                    return Err(Error::UnsupportedSql(format!(
                        "unsupported unary op {op:?}"
                    )));
                }
            }
        }
        Expr::BinaryOp { left, op, right } => eval_binary(left, op, right, row, bindings)?,
        Expr::Collate { expr, collation } => {
            // The COLLATE wrapper is transparent for value evaluation; the
            // collation only affects comparisons performed in eval_binary or
            // ORDER BY. Reject unknown collation names with SQLite's
            // wording so callers can match the error text.
            let name = collation.to_string();
            if !crate::collation::Collation::is_known(&name) {
                return Err(Error::Bind(format!("no such collation sequence: {name}")));
            }
            eval_scalar(expr, row, bindings)?
        }
        Expr::Cast {
            kind,
            expr,
            data_type,
            ..
        } => {
            let value = eval_scalar(expr, row, bindings)?;
            if crate::value::postgres_result_dialect()
                && is_boolean_expr(expr)
                && is_text_cast(data_type)
            {
                pg_bool_text(value)
            } else {
                cast_value(value, data_type, kind.clone())?
            }
        }
        Expr::Ceil { expr, .. } => match eval_scalar(expr, row, bindings)? {
            SqlValue::Null => SqlValue::Null,
            value => SqlValue::Real(numeric_value(&value)?.ceil()),
        },
        Expr::Floor { expr, .. } => match eval_scalar(expr, row, bindings)? {
            SqlValue::Null => SqlValue::Null,
            value => SqlValue::Real(numeric_value(&value)?.floor()),
        },
        Expr::Function(func) => eval_function(func, row, bindings)?,
        Expr::Like {
            negated,
            any,
            expr,
            pattern,
            escape_char,
        } => {
            if *any {
                return Err(Error::UnsupportedSql(
                    "LIKE ANY is not supported".to_owned(),
                ));
            }
            let value = eval_scalar(expr, row, bindings)?;
            let pattern = eval_scalar(pattern, row, bindings)?;
            let case_insensitive =
                crate::exec::current_connection().is_none_or(|conn| !conn.case_sensitive_like());
            like_result(
                &value,
                &pattern,
                *negated,
                escape_char.clone(),
                case_insensitive,
            )?
        }
        Expr::ILike {
            negated,
            any,
            expr,
            pattern,
            escape_char,
        } => {
            if *any {
                return Err(Error::UnsupportedSql(
                    "ILIKE ANY is not supported".to_owned(),
                ));
            }
            let value = eval_scalar(expr, row, bindings)?;
            let pattern = eval_scalar(pattern, row, bindings)?;
            ilike_result(&value, &pattern, *negated, escape_char.clone())?
        }
        // `SIMILAR TO` (https://www.postgresql.org/docs/16/functions-matching.html
        // #FUNCTIONS-SIMILARTO-REGEXP) is the SQL-standard regex flavour
        // — `%` matches any run, `_` matches one char, the pattern is
        // anchored at both ends, character classes are POSIX-style. We
        // translate it to a POSIX regex via `similar_to_regex` and then
        // delegate to the existing regex engine. Escape support mirrors
        // the LIKE escape (any single-char literal).
        Expr::SimilarTo {
            negated,
            expr,
            pattern,
            escape_char,
        } => {
            let value = eval_scalar(expr, row, bindings)?;
            let pattern_value = eval_scalar(pattern, row, bindings)?;
            similar_to_result(value, pattern_value, *negated, escape_char.clone())?
        }
        // `POSITION(sub IN str)` returns a 1-based index of the first
        // occurrence (0 if not found, NULL on NULL inputs). Same semantic
        // as SQLite's `instr(str, sub)` with swapped arg order; we re-use
        // that helper to keep behaviour aligned.
        Expr::Position { expr, r#in } => {
            let sub = eval_scalar(expr, row, bindings)?;
            let haystack = eval_scalar(r#in, row, bindings)?;
            position_result(sub, haystack)?
        }
        Expr::Between {
            expr,
            negated,
            low: low_expr,
            high: high_expr,
        } => {
            let low_collation = comparison_collation(expr, low_expr, row);
            let high_collation = comparison_collation(expr, high_expr, row);
            let value = eval_scalar(expr, row, bindings)?;
            let low = eval_scalar(low_expr, row, bindings)?;
            let high = eval_scalar(high_expr, row, bindings)?;
            if matches!(value, SqlValue::Null)
                || matches!(low, SqlValue::Null)
                || matches!(high, SqlValue::Null)
            {
                SqlValue::Null
            } else {
                // `x BETWEEN a AND b` is `x >= a AND x <= b`, each with its
                // own comparison affinity and (Q5-10) its own collation.
                let bound = |bound_expr: &Expr, bound_value: SqlValue| {
                    if affinity::may_convert(&value, &bound_value) {
                        let cmp = affinity::CmpAffinity::between(
                            affinity::expr_affinity(row, expr),
                            affinity::expr_affinity(row, bound_expr),
                        );
                        affinity::apply_pair(value.clone(), bound_value, cmp)
                    } else {
                        (value.clone(), bound_value)
                    }
                };
                let (v_low, low) = bound(low_expr, low);
                let (v_high, high) = bound(high_expr, high);
                let mut ok = compare_with_collation(&v_low, &low, low_collation.as_ref())
                    != Ordering::Less
                    && compare_with_collation(&v_high, &high, high_collation.as_ref())
                        != Ordering::Greater;
                if *negated {
                    ok = !ok;
                }
                SqlValue::Integer(if ok { 1 } else { 0 })
            }
        }
        Expr::InList {
            expr,
            list,
            negated,
        } => in_list_result(expr, list, *negated, row, bindings)?,
        Expr::Exists { subquery, negated } => {
            let exists = evaluate_subquery_exists(subquery, row, bindings)?;
            SqlValue::Integer(if exists ^ *negated { 1 } else { 0 })
        }
        Expr::InSubquery {
            expr,
            subquery,
            negated,
        } => in_subquery_result(expr, subquery, *negated, row, bindings)?,
        Expr::Subquery(subquery) => eval_subquery_value(subquery, row, bindings)?,
        Expr::IsNull(expr) => {
            crate::value::postgres_bool(matches!(eval_scalar(expr, row, bindings)?, SqlValue::Null))
        }
        Expr::IsNotNull(expr) => crate::value::postgres_bool(!matches!(
            eval_scalar(expr, row, bindings)?,
            SqlValue::Null
        )),
        Expr::IsTrue(expr) => sql_truth_result(eval_scalar(expr, row, bindings)?),
        Expr::IsNotTrue(expr) => sql_truth_result_not(eval_scalar(expr, row, bindings)?),
        Expr::IsFalse(expr) => sql_false_result(eval_scalar(expr, row, bindings)?),
        Expr::IsNotFalse(expr) => sql_false_result_not(eval_scalar(expr, row, bindings)?),
        Expr::IsUnknown(expr) => SqlValue::Integer(
            if matches!(eval_scalar(expr, row, bindings)?, SqlValue::Null) {
                1
            } else {
                0
            },
        ),
        Expr::IsNotUnknown(expr) => SqlValue::Integer(
            if !matches!(eval_scalar(expr, row, bindings)?, SqlValue::Null) {
                1
            } else {
                0
            },
        ),
        Expr::IsDistinctFrom(left, right) => {
            let (left, right) = eval_compared_pair(left, right, row, bindings)?;
            SqlValue::Integer(if is_distinct(&left, &right) { 1 } else { 0 })
        }
        Expr::IsNotDistinctFrom(left, right) => {
            let (left, right) = eval_compared_pair(left, right, row, bindings)?;
            SqlValue::Integer(if !is_distinct(&left, &right) { 1 } else { 0 })
        }
        Expr::Case {
            operand,
            conditions,
            else_result,
            ..
        } => {
            let mut evaluator = ScalarCaseEvaluator { row, bindings };
            eval_case(
                operand.as_deref(),
                conditions,
                else_result.as_deref(),
                &mut evaluator,
            )?
        }
        other => return Err(crate::sqlite_errors::unsupported_expression(other)),
    })
}
