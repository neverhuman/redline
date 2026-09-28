//! Equivalence keys: bytes that are equal exactly when two rows are equal
//! under SQLite's value comparison with the BINARY collation.
//!
//! Set operations (`UNION`, `INTERSECT`, `EXCEPT`, the recursive-CTE
//! `UNION`) and GROUP BY hash rows by these bytes. The encoding is
//! injective per row (every value carries a tag, and TEXT and BLOB carry
//! their length), and it is numerically canonical: INTEGER 1, REAL 1.0 and
//! REAL -0.0 get one key, as `1 = 1.0` and `0 = -0.0` are true in SQLite,
//! while INTEGER 9007199254740993 and REAL 9007199254740992.0 keep two
//! (SQLite compares an INTEGER with a REAL exactly). TEXT never equals a
//! number or a BLOB here, which is SQLite's rule when neither side has an
//! affinity to apply.
//!
//! The keys are only compared, never decoded; a caller that needs the
//! value keeps a representative row next to the key.

use crate::value::SqlValue;

const TAG_NULL: u8 = 0;
/// A number whose value is an integer in the i64 range: INTEGER, or a
/// finite integral REAL in [-2^63, 2^63). Payload: the i64, big-endian.
const TAG_INTEGRAL: u8 = 1;
/// Any other REAL (fractional, out of the i64 range, infinite or NaN).
/// Payload: the f64 bits, big-endian, with every NaN folded to one.
const TAG_REAL: u8 = 2;
/// Payload: the byte length as a big-endian u64, then the UTF-8 bytes.
const TAG_TEXT: u8 = 3;
/// Payload: the byte length as a big-endian u64, then the bytes.
const TAG_BLOB: u8 = 4;

/// 2^63 as an f64: the first REAL above `i64::MAX`.
const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;

/// Write the equivalence key of `row` into `out`, replacing its contents.
pub(crate) fn equiv_key_into(row: &[SqlValue], out: &mut Vec<u8>) {
    out.clear();
    for value in row {
        push_value_key(value, out);
    }
}

/// The equivalence key of `row` in a new buffer.
pub(crate) fn equiv_key(row: &[SqlValue]) -> Vec<u8> {
    let mut out = Vec::with_capacity(row.len() * 9);
    equiv_key_into(row, &mut out);
    out
}

fn push_value_key(value: &SqlValue, out: &mut Vec<u8>) {
    match value {
        SqlValue::Null => out.push(TAG_NULL),
        SqlValue::Integer(v) => push_integral(*v, out),
        SqlValue::Real(v) => match integral_value(*v) {
            Some(i) => push_integral(i, out),
            None => {
                out.push(TAG_REAL);
                let bits = if v.is_nan() {
                    f64::NAN.to_bits()
                } else {
                    v.to_bits()
                };
                out.extend_from_slice(&bits.to_be_bytes());
            }
        },
        SqlValue::Text(v) => push_bytes(TAG_TEXT, v.as_bytes(), out),
        SqlValue::Blob(v) => push_bytes(TAG_BLOB, v, out),
    }
}

fn push_integral(v: i64, out: &mut Vec<u8>) {
    out.push(TAG_INTEGRAL);
    out.extend_from_slice(&v.to_be_bytes());
}

fn push_bytes(tag: u8, bytes: &[u8], out: &mut Vec<u8>) {
    out.push(tag);
    out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// The i64 equal to `v`, when there is one. -0.0 maps to 0.
fn integral_value(v: f64) -> Option<i64> {
    if v.is_finite() && v.fract() == 0.0 && (-TWO_POW_63..TWO_POW_63).contains(&v) {
        Some(v as i64)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn text(v: &str) -> SqlValue {
        SqlValue::Text(Arc::from(v))
    }

    fn blob(v: &[u8]) -> SqlValue {
        SqlValue::Blob(Arc::from(v))
    }

    #[test]
    fn equal_rows_give_equal_keys() {
        let pairs = [
            (vec![SqlValue::Integer(1)], vec![SqlValue::Real(1.0)]),
            (vec![SqlValue::Integer(0)], vec![SqlValue::Real(-0.0)]),
            (vec![SqlValue::Real(0.0)], vec![SqlValue::Real(-0.0)]),
            (
                vec![SqlValue::Integer(i64::MIN)],
                vec![SqlValue::Real(-TWO_POW_63)],
            ),
            (
                vec![SqlValue::Integer(9_007_199_254_740_992)],
                vec![SqlValue::Real(9_007_199_254_740_992.0)],
            ),
            (
                vec![SqlValue::Integer(1), text("x"), SqlValue::Null],
                vec![SqlValue::Real(1.0), text("x"), SqlValue::Null],
            ),
            (
                vec![SqlValue::Real(f64::NAN)],
                vec![SqlValue::Real(-f64::NAN)],
            ),
            (
                vec![SqlValue::Real(f64::INFINITY)],
                vec![SqlValue::Real(f64::INFINITY)],
            ),
            (vec![blob(b"ab")], vec![blob(b"ab")]),
        ];
        for (a, b) in pairs {
            assert_eq!(equiv_key(&a), equiv_key(&b), "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn distinct_rows_give_distinct_keys() {
        let pairs = [
            // The old `T<text>|` key made these two rows one.
            (vec![text("a|Tb"), text("c")], vec![text("a"), text("b|Tc")]),
            (vec![text("ab"), text("")], vec![text("a"), text("b")]),
            (vec![blob(b"a\0b")], vec![blob(b"a")]),
            (vec![blob(b"\0")], vec![text("")]),
            (vec![blob(b"")], vec![text("")]),
            (vec![text("a")], vec![blob(b"a")]),
            (vec![text("1")], vec![SqlValue::Integer(1)]),
            (vec![SqlValue::Null], vec![text("")]),
            (vec![SqlValue::Null], vec![SqlValue::Integer(0)]),
            (vec![SqlValue::Integer(1)], vec![SqlValue::Real(1.5)]),
            (
                vec![SqlValue::Integer(9_007_199_254_740_993)],
                vec![SqlValue::Real(9_007_199_254_740_992.0)],
            ),
            (
                vec![SqlValue::Integer(i64::MAX)],
                vec![SqlValue::Real(TWO_POW_63)],
            ),
            (
                vec![SqlValue::Real(f64::INFINITY)],
                vec![SqlValue::Real(f64::NEG_INFINITY)],
            ),
            // An INTEGER whose big-endian bytes equal a REAL's bits.
            (
                vec![SqlValue::Integer(1.5f64.to_bits() as i64)],
                vec![SqlValue::Real(1.5)],
            ),
        ];
        for (a, b) in pairs {
            assert_ne!(equiv_key(&a), equiv_key(&b), "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn key_into_replaces_the_buffer() {
        let mut buf = vec![9, 9, 9];
        equiv_key_into(&[SqlValue::Integer(1)], &mut buf);
        assert_eq!(buf, equiv_key(&[SqlValue::Integer(1)]));
    }
}
