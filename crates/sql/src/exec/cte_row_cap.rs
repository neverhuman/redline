//! WS-A7 / Q5-03: how many rows of a recursive CTE the outer query needs.
//!
//! `materialize_cte` stops a recursion once it has `cap` rows. That is only
//! sound when the outer query reads the CTE row by row and keeps the first
//! LIMIT + OFFSET of them: an aggregate, a window function or a subquery
//! over the CTE needs every row, and so does another CTE of the same `WITH`
//! that reads this one. When any of that cannot be ruled out from the query
//! shape, there is no cap and the CTE is materialized in full.

use sqlparser::ast::{
    Cte, Expr, GroupByExpr, LimitClause, Query, SelectItem, SelectItemQualifiedWildcardKind,
    SetExpr, TableFactor, Value, ValueWithSpan, WildcardAdditionalOptions,
};

/// Derive an upper bound on the number of rows of CTE `cte_name` (declared
/// at `cte_index` in `ctes`) that `body_query` can read. Returns
/// `Some(limit + offset)` only when truncating the recursion there cannot
/// change the answer:
/// - the body is a single `SELECT ... FROM <cte>` (no joins, no nested
///   `WITH`, no set operation),
/// - no `WHERE`, `GROUP BY`, `HAVING`, `ORDER BY`, `DISTINCT`, `WINDOW`
///   or `QUALIFY` (each can need rows past the cap),
/// - every SELECT-list item is row-local: `*`, `t.*`, or an expression
///   built only from column names, literals, parameters, parentheses,
///   unary and binary operators, `CAST` and `COLLATE` (a function call may
///   be an aggregate or a window function; a subquery or `CASE` may read
///   the CTE again),
/// - no other CTE of the same `WITH` mentions the name (it would read the
///   truncated rows),
/// - `LIMIT` and the optional `OFFSET` are non-negative integer literals.
///
/// The cap is `limit + offset` so the trailing query can still apply its
/// OFFSET to the truncated set.
pub(super) fn derive_cte_row_cap(
    body_query: &Query,
    cte_name: &str,
    ctes: &[Cte],
    cte_index: usize,
) -> Option<usize> {
    if body_query.with.is_some()
        || body_query.order_by.is_some()
        || body_query.fetch.is_some()
        || !body_query.pipe_operators.is_empty()
    {
        return None;
    }

    let select = match body_query.body.as_ref() {
        SetExpr::Select(select) => select.as_ref(),
        _ => return None,
    };

    if select.distinct.is_some()
        || select.top.is_some()
        || select.into.is_some()
        || select.prewhere.is_some()
        || select.selection.is_some()
        || select.having.is_some()
        || select.qualify.is_some()
        || !select.named_window.is_empty()
        || !select.lateral_views.is_empty()
        || !select.connect_by.is_empty()
        || !select.cluster_by.is_empty()
        || !select.distribute_by.is_empty()
        || !select.sort_by.is_empty()
        || !matches!(&select.group_by, GroupByExpr::Expressions(exprs, _) if exprs.is_empty())
        || select.from.len() != 1
        || !select.from[0].joins.is_empty()
    {
        return None;
    }

    // Confirm the single FROM source is exactly the recursive CTE.
    let factor = &select.from[0].relation;
    let from_name = match factor {
        TableFactor::Table { name, .. } => name.0.last().and_then(|part| match part {
            sqlparser::ast::ObjectNamePart::Identifier(ident) => Some(&ident.value),
            _ => None,
        })?,
        _ => return None,
    };
    if !from_name.eq_ignore_ascii_case(cte_name) {
        return None;
    }

    if !select.projection.iter().all(select_item_is_row_local) {
        return None;
    }

    // Extract numeric LIMIT + OFFSET from a `LimitOffset` form.
    let (limit_expr, offset_expr) = match body_query.limit_clause.as_ref()? {
        LimitClause::LimitOffset {
            limit,
            offset,
            limit_by,
        } if limit_by.is_empty() => (limit.as_ref(), offset.as_ref().map(|o| &o.value)),
        LimitClause::OffsetCommaLimit { offset, limit } => (Some(limit), Some(offset)),
        _ => return None,
    };

    let limit_value = literal_u64(limit_expr?)?;
    let offset_value = match offset_expr {
        Some(expr) => literal_u64(expr)?,
        None => 0,
    };

    if sibling_mentions_name(ctes, cte_index, cte_name) {
        return None;
    }

    let cap = limit_value.checked_add(offset_value)?;
    // Saturate to usize to avoid pathological cap values on 32-bit.
    usize::try_from(cap).ok()
}

fn select_item_is_row_local(item: &SelectItem) -> bool {
    match item {
        SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } => {
            expr_is_row_local(expr)
        }
        SelectItem::Wildcard(options) => wildcard_is_row_local(options),
        SelectItem::QualifiedWildcard(SelectItemQualifiedWildcardKind::ObjectName(_), options) => {
            wildcard_is_row_local(options)
        }
        SelectItem::QualifiedWildcard(SelectItemQualifiedWildcardKind::Expr(_), _) => false,
    }
}

/// `* REPLACE (expr AS c)` carries expressions; the other wildcard
/// options only pick or rename columns.
fn wildcard_is_row_local(options: &WildcardAdditionalOptions) -> bool {
    options.opt_replace.is_none()
}

/// True when `expr` is computed from the current row alone. Deliberately
/// an allow-list: anything not listed (function calls, subqueries,
/// `EXISTS`, `IN (SELECT ...)`, `CASE`, ...) may aggregate, open a window
/// or read the CTE again.
fn expr_is_row_local(expr: &Expr) -> bool {
    match expr {
        Expr::Identifier(_) | Expr::CompoundIdentifier(_) | Expr::Value(_) => true,
        Expr::Nested(inner)
        | Expr::UnaryOp { expr: inner, .. }
        | Expr::Cast { expr: inner, .. }
        | Expr::Collate { expr: inner, .. } => expr_is_row_local(inner),
        Expr::BinaryOp { left, right, .. } => expr_is_row_local(left) && expr_is_row_local(right),
        _ => false,
    }
}

/// True when a CTE other than `ctes[cte_index]` mentions `name` anywhere in
/// its body. This is a word match over the rendered SQL, so a string
/// literal or an unrelated column spelled like the CTE also counts; a false
/// match only costs the pushdown, never an answer.
fn sibling_mentions_name(ctes: &[Cte], cte_index: usize, name: &str) -> bool {
    ctes.iter()
        .enumerate()
        .filter(|(index, _)| *index != cte_index)
        .any(|(_, cte)| contains_word_ci(&cte.query.to_string(), name))
}

fn contains_word_ci(haystack: &str, word: &str) -> bool {
    let hay = haystack.as_bytes();
    let needle = word.as_bytes();
    if needle.is_empty() || needle.len() > hay.len() {
        return false;
    }
    let is_word_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80;
    (0..=hay.len() - needle.len()).any(|start| {
        let end = start + needle.len();
        hay[start..end].eq_ignore_ascii_case(needle)
            && (start == 0 || !is_word_byte(hay[start - 1]))
            && (end == hay.len() || !is_word_byte(hay[end]))
    })
}

fn literal_u64(expr: &Expr) -> Option<u64> {
    match expr {
        Expr::Value(ValueWithSpan { value, .. }) => match value {
            Value::Number(text, _) => text.parse::<u64>().ok(),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlparser::ast::Statement;
    use sqlparser::dialect::SQLiteDialect;
    use sqlparser::parser::Parser;

    /// The cap of the first CTE of `sql` for the statement's main query.
    fn cap(sql: &str) -> Option<usize> {
        let mut statements = Parser::parse_sql(&SQLiteDialect {}, sql).expect("parse");
        let Statement::Query(mut query) = statements.remove(0) else {
            panic!("not a query: {sql}");
        };
        let with = query.with.take().expect("WITH clause");
        let name = with.cte_tables[0].alias.name.value.clone();
        derive_cte_row_cap(&query, &name, &with.cte_tables, 0)
    }

    const C: &str = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c)";

    #[test]
    fn row_local_projection_keeps_the_cap() {
        assert_eq!(cap(&format!("{C} SELECT x FROM c LIMIT 10")), Some(10));
        assert_eq!(
            cap(&format!("{C} SELECT * FROM c LIMIT 5 OFFSET 3")),
            Some(8)
        );
        assert_eq!(cap(&format!("{C} SELECT c.* FROM c LIMIT 3, 2")), Some(5));
        assert_eq!(
            cap(&format!(
                "{C} SELECT -x, (x * 2) AS y, CAST(x AS TEXT), x COLLATE NOCASE, c.x, ? FROM c LIMIT 1"
            )),
            Some(1)
        );
    }

    #[test]
    fn aggregate_window_subquery_or_call_drops_the_cap() {
        for tail in [
            "SELECT count(*) FROM c LIMIT 1",
            "SELECT sum(x) OVER () FROM c LIMIT 1",
            "SELECT x, count(*) OVER w FROM c WINDOW w AS () LIMIT 1",
            "SELECT (SELECT count(*) FROM c) FROM c LIMIT 1",
            "SELECT EXISTS (SELECT 1 FROM c WHERE x > 3) FROM c LIMIT 1",
            "SELECT x IN (SELECT x FROM c) FROM c LIMIT 1",
            "SELECT CASE WHEN x > 0 THEN x END FROM c LIMIT 1",
            "SELECT abs(x) FROM c LIMIT 1",
            "SELECT x + max(x) OVER () FROM c LIMIT 1",
        ] {
            assert_eq!(cap(&format!("{C} {tail}")), None, "{tail}");
        }
    }

    #[test]
    fn filters_and_orderings_drop_the_cap() {
        for tail in [
            "SELECT x FROM c WHERE x > 1 LIMIT 1",
            "SELECT x FROM c ORDER BY x LIMIT 1",
            "SELECT DISTINCT x FROM c LIMIT 1",
            "SELECT x FROM c GROUP BY x LIMIT 1",
            "SELECT x FROM c, c AS d LIMIT 1",
            "SELECT x FROM c LIMIT ?",
            "SELECT x FROM c",
        ] {
            assert_eq!(cap(&format!("{C} {tail}")), None, "{tail}");
        }
    }

    #[test]
    fn a_sibling_cte_that_reads_the_cte_drops_the_cap() {
        let sibling = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c), \
                       d(n) AS (SELECT (SELECT count(*) FROM c)) SELECT x FROM c LIMIT 1";
        assert_eq!(cap(sibling), None);
        let unrelated = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c), \
                         d(n) AS (SELECT 1 FROM cc) SELECT x FROM c LIMIT 1";
        assert_eq!(cap(unrelated), Some(1));
    }

    #[test]
    fn word_match_needs_identifier_boundaries() {
        assert!(contains_word_ci("SELECT count(*) FROM C", "c"));
        assert!(contains_word_ci("SELECT \"c\".x", "c"));
        assert!(!contains_word_ci("SELECT cc, c_1, xc, $c FROM t", "c"));
        assert!(!contains_word_ci("", "c"));
    }
}
