//! SQLite numeric semantics in one place (launch Q5-06 / S9-06).
//!
//! * Integer `+ - *` that overflows i64 is recomputed in REAL, like
//!   SQLite's `OP_Add`/`OP_Subtract`/`OP_Multiply` (`goto fp_math`).
//!   `MIN / -1` is also REAL, `MIN % -1` is 0, and division or remainder
//!   by zero is NULL. Under the Postgres dialect an overflow is the error
//!   `bigint out of range` instead.
//! * REAL `%` converts both operands to INTEGER first, as SQLite does, and a
//!   REAL result that is NaN is NULL.
//! * Unary minus of `i64::MIN` is REAL; `abs(i64::MIN)` raises
//!   `integer overflow`.
//! * INTEGER/REAL comparison is exact ([`int_real_cmp`]) and TEXT/BLOB truth
//!   uses the numeric prefix ([`sqlite_numeric_prefix`]); both live in the
//!   kernel so its CHECK/default evaluator shares them.
//! * [`SumAcc`] is SQLite's `sum()`/`total()`/`avg()` accumulator.

use crate::error::{Error, Result};
use crate::value::SqlValue;

pub(crate) use redlinedb_kernel::catalog::{int_real_cmp, sqlite_text_is_true};

// An explicit path so tests that re-include this file with `#[path]` (the
// ScalarProgram VM tests) resolve the same submodule.
#[path = "numeric/sum_acc.rs"]
mod sum_acc;
pub(crate) use sum_acc::{IntRun, SumAcc, SumParts};
#[path = "numeric/text_number.rs"]
pub(crate) mod text_number;

/// A binary arithmetic operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

/// What INTEGER arithmetic produced. Replaces the old `Option<i64>`
/// contract, where `None` meant NULL and so turned `MIN / -1` into NULL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IntOutcome {
    /// The exact result fits in i64.
    Value(i64),
    /// Division or remainder by zero: SQL NULL.
    Null,
    /// The exact result does not fit in i64: SQLite recomputes it in REAL.
    Promote,
}

/// INTEGER arithmetic with SQLite's overflow rules.
pub(crate) fn int_op(op: ArithOp, a: i64, b: i64) -> IntOutcome {
    let checked = match op {
        ArithOp::Add => a.checked_add(b),
        ArithOp::Sub => a.checked_sub(b),
        ArithOp::Mul => a.checked_mul(b),
        ArithOp::Div => {
            if b == 0 {
                return IntOutcome::Null;
            }
            // Only MIN / -1 fails, and SQLite answers it in REAL.
            a.checked_div(b)
        }
        ArithOp::Rem => {
            if b == 0 {
                return IntOutcome::Null;
            }
            // SQLite rewrites a divisor of -1 to 1, so MIN % -1 is 0.
            return IntOutcome::Value(if b == -1 { 0 } else { a % b });
        }
    };
    checked.map_or(IntOutcome::Promote, IntOutcome::Value)
}

/// REAL arithmetic as SQLite's `fp_math` path computes it. `None` is NULL.
/// The dialect is only consulted for `%` and for a NaN result, so plain
/// REAL arithmetic never reads the environment.
pub(crate) fn real_op(op: ArithOp, a: f64, b: f64) -> Option<f64> {
    let result = match op {
        ArithOp::Add => a + b,
        ArithOp::Sub => a - b,
        ArithOp::Mul => a * b,
        ArithOp::Div => {
            if b == 0.0 {
                return None;
            }
            a / b
        }
        ArithOp::Rem => {
            if crate::value::postgres_result_dialect() {
                if b == 0.0 {
                    return None;
                }
                a % b
            } else {
                // SQLite: `%` casts both operands to INTEGER, and the result
                // keeps the REAL storage class.
                let divisor = real_to_i64(b);
                if divisor == 0 {
                    return None;
                }
                let divisor = if divisor == -1 { 1 } else { divisor };
                (real_to_i64(a) % divisor) as f64
            }
        }
    };
    if result.is_nan() && !crate::value::postgres_result_dialect() {
        return None;
    }
    Some(result)
}

/// `sqlite3RealToI64`: clamp to the i64 range, truncating toward zero.
fn real_to_i64(r: f64) -> i64 {
    if r < -9_223_372_036_854_774_784.0 {
        i64::MIN
    } else if r > 9_223_372_036_854_774_784.0 {
        i64::MAX
    } else {
        r as i64
    }
}

/// The error an INTEGER overflow raises where SQLite has no REAL fallback
/// (`abs(MIN)`), or anywhere under the Postgres dialect.
pub(crate) fn overflow_error() -> Error {
    if crate::value::postgres_result_dialect() {
        Error::BigintOutOfRange
    } else {
        Error::IntegerOverflow
    }
}

/// `a op b` for two INTEGERs: exact when it fits, else REAL (SQLite) or
/// `bigint out of range` (Postgres).
pub(crate) fn int_arith(op: ArithOp, a: i64, b: i64) -> Result<SqlValue> {
    match int_op(op, a, b) {
        IntOutcome::Value(v) => Ok(SqlValue::Integer(v)),
        IntOutcome::Null => Ok(SqlValue::Null),
        IntOutcome::Promote => {
            if crate::value::postgres_result_dialect() {
                return Err(Error::BigintOutOfRange);
            }
            Ok(real_value(op, a as f64, b as f64))
        }
    }
}

fn real_value(op: ArithOp, a: f64, b: f64) -> SqlValue {
    match real_op(op, a, b) {
        Some(v) => SqlValue::Real(v),
        None => SqlValue::Null,
    }
}

/// Binary arithmetic over two SQL values. NULL propagates; INTEGER pairs
/// use [`int_arith`]; any REAL operand switches to REAL arithmetic.
///
/// A TEXT or BLOB operand is read the way SQLite reads it
/// ([`text_number::arith_operand`]): `'1' + 2` is INTEGER 3, `'1abc' + 1`
/// is 2, `'1e2' + 1` is REAL 101.0 and `'abc' + 1` is 1. Under the Postgres
/// dialect two TEXT operands are read as REAL and any other TEXT/BLOB mix is
/// a datatype mismatch, as before (Postgres refuses `'abc' + 1`).
pub(crate) fn arith(op: ArithOp, left: SqlValue, right: SqlValue) -> Result<SqlValue> {
    if !crate::value::postgres_result_dialect() {
        return arith_numbers(op, sqlite_operand(left), sqlite_operand(right));
    }
    arith_numbers(op, left, right)
}

/// A TEXT or BLOB operand as the number SQLite's arithmetic reads it.
fn sqlite_operand(value: SqlValue) -> SqlValue {
    match value {
        SqlValue::Text(text) => text_number::arith_operand(text.as_bytes()),
        SqlValue::Blob(bytes) => text_number::arith_operand(&bytes),
        other => other,
    }
}

fn arith_numbers(op: ArithOp, left: SqlValue, right: SqlValue) -> Result<SqlValue> {
    match (left, right) {
        (SqlValue::Null, _) | (_, SqlValue::Null) => Ok(SqlValue::Null),
        (SqlValue::Integer(a), SqlValue::Integer(b)) => int_arith(op, a, b),
        (SqlValue::Integer(a), SqlValue::Real(b)) => Ok(real_value(op, a as f64, b)),
        (SqlValue::Real(a), SqlValue::Integer(b)) => Ok(real_value(op, a, b as f64)),
        (SqlValue::Real(a), SqlValue::Real(b)) => Ok(real_value(op, a, b)),
        (SqlValue::Text(a), SqlValue::Text(b)) => {
            let a = a
                .trim()
                .parse::<f64>()
                .map_err(|_| Error::DatatypeMismatch)?;
            let b = b
                .trim()
                .parse::<f64>()
                .map_err(|_| Error::DatatypeMismatch)?;
            Ok(real_value(op, a, b))
        }
        _ => Err(Error::DatatypeMismatch),
    }
}

/// Unary minus: `-MIN` does not fit, so SQLite answers REAL 2^63. SQLite
/// computes `-x` as `0 - x`, so a TEXT or BLOB operand is read as a number
/// first (`-'5'` is -5, `-'abc'` is 0); the Postgres dialect still refuses it.
pub(crate) fn negate(value: SqlValue) -> Result<SqlValue> {
    let value = if crate::value::postgres_result_dialect() {
        value
    } else {
        sqlite_operand(value)
    };
    match value {
        SqlValue::Integer(v) => match v.checked_neg() {
            Some(n) => Ok(SqlValue::Integer(n)),
            None if crate::value::postgres_result_dialect() => Err(Error::BigintOutOfRange),
            None => Ok(SqlValue::Real(-(v as f64))),
        },
        SqlValue::Real(v) => Ok(SqlValue::Real(-v)),
        SqlValue::Null => Ok(SqlValue::Null),
        _ => Err(Error::DatatypeMismatch),
    }
}

/// Unary minus for planner constant folding, where there is no error path:
/// `None` means "not a foldable number".
pub(crate) fn negate_constant(value: SqlValue) -> Option<SqlValue> {
    match value {
        SqlValue::Integer(v) => Some(match v.checked_neg() {
            Some(n) => SqlValue::Integer(n),
            None => SqlValue::Real(-(v as f64)),
        }),
        SqlValue::Real(v) => Some(SqlValue::Real(-v)),
        _ => None,
    }
}

/// `abs()` of an INTEGER: `abs(MIN)` raises `integer overflow` in SQLite.
pub(crate) fn abs_i64(v: i64) -> Result<SqlValue> {
    v.checked_abs()
        .map(SqlValue::Integer)
        .ok_or_else(overflow_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_op_promotes_or_nulls_like_sqlite() {
        assert_eq!(int_op(ArithOp::Add, i64::MAX, 1), IntOutcome::Promote);
        assert_eq!(int_op(ArithOp::Sub, i64::MIN, 1), IntOutcome::Promote);
        assert_eq!(int_op(ArithOp::Mul, i64::MAX, 2), IntOutcome::Promote);
        assert_eq!(int_op(ArithOp::Div, i64::MIN, -1), IntOutcome::Promote);
        assert_eq!(int_op(ArithOp::Rem, i64::MIN, -1), IntOutcome::Value(0));
        assert_eq!(int_op(ArithOp::Div, 7, 0), IntOutcome::Null);
        assert_eq!(int_op(ArithOp::Rem, 7, 0), IntOutcome::Null);
        assert_eq!(int_op(ArithOp::Rem, -7, 3), IntOutcome::Value(-1));
        assert_eq!(int_op(ArithOp::Div, -7, 2), IntOutcome::Value(-3));
    }

    #[test]
    fn real_remainder_casts_operands_to_integer() {
        assert_eq!(real_op(ArithOp::Rem, 5.5, 2.0), Some(1.0));
        assert_eq!(real_op(ArithOp::Rem, 7.0, 2.5), Some(1.0));
        assert_eq!(real_op(ArithOp::Rem, 7.0, 0.5), None);
        assert_eq!(real_op(ArithOp::Sub, f64::INFINITY, f64::INFINITY), None);
    }

    #[test]
    fn arith_promotes_min_div_minus_one() {
        assert_eq!(
            arith(
                ArithOp::Div,
                SqlValue::Integer(i64::MIN),
                SqlValue::Integer(-1)
            )
            .unwrap(),
            SqlValue::Real(9_223_372_036_854_775_808.0)
        );
        assert_eq!(
            negate(SqlValue::Integer(i64::MIN)).unwrap(),
            SqlValue::Real(9_223_372_036_854_775_808.0)
        );
        assert_eq!(abs_i64(i64::MIN), Err(Error::IntegerOverflow));
    }
}
