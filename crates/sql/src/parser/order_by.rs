//! NEW-01: binding the ORDER BY clause of a SELECT, and in particular the
//! terms that name a result column by position (`ORDER BY 2`).
//!
//! SQLite reads a term as a position when, after dropping parentheses and
//! top-level `COLLATE`s, it is an integer literal that fits in 32 bits,
//! optionally under unary `+` or `-` (`sqlite3ExprIsInteger`): `2`, `+2`,
//! `(2)`, `- -2` and `2 COLLATE NOCASE` are positions, `2 + 0`, `2.0` and
//! `'2'` are constants. A position outside 1..=N (N result columns) is an
//! error, 0 and negative positions included; a literal too large for 32
//! bits is a constant. A position sorts by the result column itself, with
//! the term's collation or else the result expression's own `COLLATE`.
//!
//! A bound position is a bare integer literal (`Expr::Value`), under an
//! `Expr::Collate` when a collation applies, and every ORDER BY executor
//! reads such a term as the result column
//! (`crate::exec::order_position::order_position`). Any other term that
//! normalizes to a number literal (`1 + 0` folds to `1`) is wrapped in
//! `Expr::Nested`, so no executor takes a constant for a position. When
//! the result column is an unaliased column reference that no other result
//! column shadows, the term becomes that column reference instead, so an
//! index on the column can still satisfy the order.

use sqlparser::ast::{
    Expr, Ident, ObjectName, OrderBy, OrderByExpr, OrderByKind, SelectItem, UnaryOperator, Value,
    ValueWithSpan,
};

use super::normalize_expr;
use crate::error::{Error, Result};
use crate::statement::ParamLayout;

/// What the binder knows about the SELECT list the positions refer to.
pub(crate) struct ResultShape<'a> {
    /// The SELECT list as bound. An empty list means every source column
    /// (compound, VALUES and wrapper queries pass none at all).
    pub projection: &'a [SelectItem],
    pub distinct: bool,
}

/// Bind `order_by` for a SELECT whose result columns are `output_columns`.
pub(crate) fn bind_order_by(
    order_by: Option<OrderBy>,
    output_columns: &[String],
    shape: Option<ResultShape<'_>>,
    params: &mut ParamLayout,
) -> Result<Vec<OrderByExpr>> {
    let Some(order_by) = order_by else {
        return Ok(Vec::new());
    };
    let terms = match order_by.kind {
        OrderByKind::Expressions(terms) => terms,
        OrderByKind::All(_) => {
            return Err(Error::UnsupportedSql(
                "ORDER BY ALL is not supported".to_owned(),
            ));
        }
    };
    let mut out = Vec::with_capacity(terms.len());
    for (index, term) in terms.into_iter().enumerate() {
        let OrderByExpr {
            expr,
            options,
            with_fill,
        } = term;
        let expr = match sqlite_integer_term(&expr) {
            Some(position) => {
                bind_position(position, index, &expr, output_columns, shape.as_ref())?
            }
            None => guard_constant(normalize_expr(expr, params)?),
        };
        out.push(OrderByExpr {
            expr,
            options,
            with_fill,
        });
    }
    Ok(out)
}

/// The integer SQLite reads from an ORDER BY term, if it reads one.
fn sqlite_integer_term(expr: &Expr) -> Option<i64> {
    let mut inner = expr;
    while let Expr::Nested(next) | Expr::Collate { expr: next, .. } = inner {
        inner = next;
    }
    sqlite_expr_is_integer(inner)
}

/// `sqlite3ExprIsInteger`: a 32-bit integer literal under any number of
/// parentheses and unary `+`/`-`.
fn sqlite_expr_is_integer(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Value(ValueWithSpan {
            value: Value::Number(text, _),
            ..
        }) => text.parse::<i32>().ok().map(i64::from),
        Expr::Nested(inner)
        | Expr::UnaryOp {
            op: UnaryOperator::Plus,
            expr: inner,
        } => sqlite_expr_is_integer(inner),
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr: inner,
        } => sqlite_expr_is_integer(inner).map(|v| -v),
        _ => None,
    }
}

fn bind_position(
    position: i64,
    index: usize,
    term: &Expr,
    output_columns: &[String],
    shape: Option<&ResultShape<'_>>,
) -> Result<Expr> {
    let width = output_columns.len();
    let column = usize::try_from(position)
        .ok()
        .and_then(|p| p.checked_sub(1))
        .filter(|column| *column < width)
        .ok_or_else(|| {
            Error::Bind(format!(
                "{} ORDER BY term out of range - should be between 1 and {width}",
                ordinal_word(index + 1)
            ))
        })?;
    let item = shape.and_then(|shape| result_item(shape.projection, column, width));
    let collation = top_collation(term).or_else(|| item.and_then(top_collation));
    let base = match shape.and_then(|shape| unshadowed_column_ref(shape, column, width)) {
        Some(ident) => Expr::Identifier(ident),
        None => position_literal(column + 1),
    };
    Ok(match collation {
        Some(collation) => Expr::Collate {
            expr: Box::new(base),
            collation,
        },
        None => base,
    })
}

/// The expression behind result column `column`, when the SELECT list maps
/// one item to one column (no `*`).
fn result_item(projection: &[SelectItem], column: usize, width: usize) -> Option<&Expr> {
    if projection.len() != width {
        return None;
    }
    match projection.get(column)? {
        SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } => Some(expr),
        SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(_, _) => None,
    }
}

/// The column name to sort by instead of the position, when every ORDER BY
/// executor resolves that name to exactly this result column: the item is
/// an unaliased `name` (not a rowid alias), the query is not DISTINCT, and
/// no other item is a `*`, is aliased `name`, or is a reference to a
/// column called `name`.
fn unshadowed_column_ref(shape: &ResultShape<'_>, column: usize, width: usize) -> Option<Ident> {
    if shape.distinct
        || shape.projection.len() != width
        || shape.projection.iter().any(|item| {
            matches!(
                item,
                SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(_, _)
            )
        })
    {
        return None;
    }
    let SelectItem::UnnamedExpr(Expr::Identifier(ident)) = shape.projection.get(column)? else {
        return None;
    };
    let name = ident.value.as_str();
    if ["rowid", "_rowid_", "oid"]
        .iter()
        .any(|rowid| rowid.eq_ignore_ascii_case(name))
    {
        return None;
    }
    let shadowed = shape
        .projection
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != column)
        .any(|(_, item)| match item {
            SelectItem::ExprWithAlias { alias, expr } => {
                alias.value.eq_ignore_ascii_case(name) || names_column(expr, name)
            }
            SelectItem::UnnamedExpr(expr) => names_column(expr, name),
            SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(_, _) => true,
        });
    (!shadowed).then(|| ident.clone())
}

fn names_column(expr: &Expr, name: &str) -> bool {
    match expr {
        Expr::Identifier(ident) => ident.value.eq_ignore_ascii_case(name),
        Expr::CompoundIdentifier(parts) => parts
            .last()
            .is_some_and(|part| part.value.eq_ignore_ascii_case(name)),
        Expr::Nested(inner) => names_column(inner, name),
        _ => false,
    }
}

/// The collation of an explicit top-level `COLLATE` (under parentheses).
fn top_collation(expr: &Expr) -> Option<ObjectName> {
    match expr {
        Expr::Collate { collation, .. } => Some(collation.clone()),
        Expr::Nested(inner) => top_collation(inner),
        _ => None,
    }
}

fn position_literal(position: usize) -> Expr {
    Expr::Value(ValueWithSpan::from(Value::Number(
        position.to_string(),
        false,
    )))
}

/// Keep a constant that folded to a number literal from reading as a
/// position.
fn guard_constant(expr: Expr) -> Expr {
    match expr {
        Expr::Value(ValueWithSpan {
            value: Value::Number(..),
            ..
        }) => Expr::Nested(Box::new(expr)),
        Expr::Collate { expr, collation } => Expr::Collate {
            expr: Box::new(guard_constant(*expr)),
            collation,
        },
        other => other,
    }
}

pub(crate) fn ordinal_word(n: usize) -> String {
    let mod100 = n % 100;
    if (11..=13).contains(&mod100) {
        return format!("{n}th");
    }
    let suffix = match n % 10 {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
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

    fn parse_projection(text: &str) -> Vec<SelectItem> {
        Parser::new(&SQLiteDialect {})
            .try_with_sql(text)
            .expect("parser init")
            .parse_projection()
            .expect("parse projection")
    }

    fn bind(
        terms: &[&str],
        columns: &[&str],
        projection: &str,
        distinct: bool,
    ) -> Result<Vec<Expr>> {
        let order_by = OrderBy {
            kind: OrderByKind::Expressions(
                terms
                    .iter()
                    .map(|term| OrderByExpr {
                        expr: parse_expr(term),
                        options: sqlparser::ast::OrderByOptions::default(),
                        with_fill: None,
                    })
                    .collect(),
            ),
            interpolate: None,
        };
        let columns: Vec<String> = columns.iter().map(|c| (*c).to_owned()).collect();
        let projection = parse_projection(projection);
        let mut params = ParamLayout::default();
        let shape = ResultShape {
            projection: &projection,
            distinct,
        };
        Ok(
            bind_order_by(Some(order_by), &columns, Some(shape), &mut params)?
                .into_iter()
                .map(|term| term.expr)
                .collect(),
        )
    }

    #[test]
    fn sqlite_reads_these_terms_as_integers() {
        for (text, want) in [
            ("1", Some(1)),
            ("42", Some(42)),
            ("+1", Some(1)),
            ("(1)", Some(1)),
            ("((2))", Some(2)),
            ("- -1", Some(1)),
            ("-(-1)", Some(1)),
            ("-1", Some(-1)),
            ("0", Some(0)),
            ("1 COLLATE NOCASE", Some(1)),
            ("(1 COLLATE NOCASE)", Some(1)),
            ("2147483647", Some(2_147_483_647)),
            ("2147483648", None),
            ("1 + 0", None),
            ("1.0", None),
            ("'1'", None),
            ("x", None),
            ("?", None),
        ] {
            assert_eq!(sqlite_integer_term(&parse_expr(text)), want, "{text}");
        }
    }

    #[test]
    fn positions_bind_to_literals_or_unshadowed_columns() {
        let got = bind(&["2", "1"], &["-x", "y"], "-x, y", false).expect("bind");
        assert_eq!(got[0], parse_expr("y"));
        assert_eq!(got[1], parse_expr("1"));
        // DISTINCT keeps the position: its executor resolves names by alias.
        let got = bind(&["2"], &["-x", "y"], "-x, y", true).expect("bind");
        assert_eq!(got[0], parse_expr("2"));
        // An alias or another column named `y` shadows it.
        let got = bind(&["2"], &["y", "y"], "x AS y, y", false).expect("bind");
        assert_eq!(got[0], parse_expr("2"));
        let got = bind(&["2"], &["y", "y"], "t.y, y", false).expect("bind");
        assert_eq!(got[0], parse_expr("2"));
        let got = bind(&["1"], &["rowid"], "rowid", false).expect("bind");
        assert_eq!(got[0], parse_expr("1"));
    }

    #[test]
    fn positions_keep_the_term_or_result_collation() {
        let got = bind(&["1 COLLATE NOCASE"], &["y"], "y", false).expect("bind");
        assert_eq!(got[0], parse_expr("y COLLATE NOCASE"));
        let got = bind(&["1"], &["g"], "g COLLATE NOCASE AS g", false).expect("bind");
        assert_eq!(got[0], parse_expr("1 COLLATE NOCASE"));
        let got = bind(&["(1) COLLATE RTRIM"], &["g"], "g COLLATE NOCASE", false).expect("bind");
        assert_eq!(got[0], parse_expr("1 COLLATE RTRIM"));
    }

    #[test]
    fn constants_are_not_positions() {
        let got = bind(&["1 + 0", "1.0", "2147483648"], &["x"], "x", false).expect("bind");
        assert_eq!(got[0], Expr::Nested(Box::new(parse_expr("1"))));
        assert!(matches!(&got[1], Expr::Nested(_)));
        assert!(matches!(&got[2], Expr::Nested(_)));
    }

    #[test]
    fn out_of_range_positions_fail_like_sqlite() {
        for (terms, want) in [
            (
                vec!["2"],
                "1st ORDER BY term out of range - should be between 1 and 1",
            ),
            (
                vec!["0"],
                "1st ORDER BY term out of range - should be between 1 and 1",
            ),
            (
                vec!["-1"],
                "1st ORDER BY term out of range - should be between 1 and 1",
            ),
            (
                vec!["x", "(3)"],
                "2nd ORDER BY term out of range - should be between 1 and 1",
            ),
        ] {
            let err = bind(&terms, &["x"], "x", false).expect_err("out of range");
            assert!(err.to_string().contains(want), "{terms:?}: {err}");
        }
    }

    #[test]
    fn ordinal_words() {
        for (n, want) in [
            (1, "1st"),
            (2, "2nd"),
            (3, "3rd"),
            (4, "4th"),
            (11, "11th"),
            (12, "12th"),
            (13, "13th"),
            (21, "21st"),
            (111, "111th"),
        ] {
            assert_eq!(ordinal_word(n), want);
        }
    }
}
