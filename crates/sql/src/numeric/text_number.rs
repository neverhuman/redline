//! SQLite's readings of a TEXT (or the bytes of a BLOB) as a number.
//!
//! Each function ports one SQLite routine, because they disagree on
//! purpose:
//!
//! | input    | [`arith_operand`] | [`numerify`] | [`comparison_number`] |
//! |----------|-------------------|--------------|-----------------------|
//! | `'1'`    | INTEGER 1         | INTEGER 1    | INTEGER 1             |
//! | `'1.0'`  | REAL 1.0          | INTEGER 1    | REAL 1.0              |
//! | `'1e2'`  | REAL 100.0        | INTEGER 100  | REAL 100.0            |
//! | `'12abc'`| INTEGER 12        | INTEGER 12   | stays TEXT            |
//! | `'abc'`  | INTEGER 0         | INTEGER 0    | stays TEXT            |
//! | `'inf'`  | INTEGER 0         | INTEGER 0    | stays TEXT            |
//!
//! * [`arith_operand`] is `computeNumericType` (vdbe.c): an operand of
//!   `+ - * / %` and of unary minus.
//! * [`numerify`] is `sqlite3VdbeMemNumerify`: `CAST(x AS NUMERIC)` and every
//!   cast whose type name has NUMERIC affinity (`DATE`, `BOOLEAN`, ...).
//! * [`comparison_number`] is `applyNumericAffinity(p, 0)`: comparison
//!   affinity converts only a text that is a well-formed number.
//!
//! Rust's `str::parse::<f64>` also reads `inf`, `NaN` and `infinity`, and
//! `str::trim` strips Unicode spaces; SQLite reads none of those, so nothing
//! here uses either on untrusted text.

use redlinedb_kernel::catalog::sqlite_numeric_prefix;

use crate::value::SqlValue;

/// What `sqlite3AtoF` reports about a byte string.
#[derive(Debug, Clone, Copy)]
struct AtoF {
    /// `1` a whole-string integer, `2`/`3` a whole-string number with a
    /// decimal point and/or exponent, `-1` a valid prefix with a decimal
    /// point or exponent followed by other text, `0` anything else.
    rc: i32,
    /// The value of the numeric prefix (0.0 when there is none).
    value: f64,
}

/// `sqlite3Isspace`: space, tab, newline, vertical tab, form feed, CR.
fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// The classification half of `sqlite3AtoF` (util.c) for UTF-8 input.
fn atof(bytes: &[u8]) -> AtoF {
    let n = bytes.len();
    let mut i = 0;
    while i < n && is_space(bytes[i]) {
        i += 1;
    }
    if i >= n {
        return AtoF { rc: 0, value: 0.0 };
    }
    if bytes[i] == b'-' || bytes[i] == b'+' {
        i += 1;
    }
    let mut digits = 0usize;
    let mut e_type = 1;
    let mut e_valid = true;
    while i < n && bytes[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    let mut at_end = i >= n;
    if !at_end && bytes[i] == b'.' {
        i += 1;
        e_type += 1;
        while i < n && bytes[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
        at_end = i >= n;
    }
    if !at_end && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        e_valid = false;
        e_type += 1;
        if i < n {
            if bytes[i] == b'-' || bytes[i] == b'+' {
                i += 1;
            }
            while i < n && bytes[i].is_ascii_digit() {
                i += 1;
                e_valid = true;
            }
        }
        at_end = i >= n;
    }
    if !at_end {
        while i < n && is_space(bytes[i]) {
            i += 1;
        }
    }
    let rc = if i == n && digits > 0 && e_valid {
        e_type
    } else if e_type >= 2 && (e_type == 3 || e_valid) && digits > 0 {
        -1
    } else {
        0
    };
    AtoF {
        rc,
        value: sqlite_numeric_prefix(bytes),
    }
}

/// `sqlite3Atoi64` (util.c) for UTF-8 input: `(rc, value)` where rc is 0
/// for a whole-string integer that fits, 1 for extra text after the digits,
/// -1 for no digits, 2 for an overflow, 3 for exactly 2^63 without a sign.
fn atoi64(bytes: &[u8]) -> (i32, i64) {
    let n = bytes.len();
    let mut i = 0;
    while i < n && is_space(bytes[i]) {
        i += 1;
    }
    let mut neg = false;
    if i < n {
        if bytes[i] == b'-' {
            neg = true;
            i += 1;
        } else if bytes[i] == b'+' {
            i += 1;
        }
    }
    let start = i;
    while i < n && bytes[i] == b'0' {
        i += 1;
    }
    let digits_start = i;
    let mut u: u64 = 0;
    while i < n && bytes[i].is_ascii_digit() {
        u = u.wrapping_mul(10).wrapping_add(u64::from(bytes[i] - b'0'));
        i += 1;
    }
    let digit_count = i - digits_start;
    let value = if neg {
        (u as i64).wrapping_neg()
    } else {
        u as i64
    };
    let mut rc = 0;
    if digit_count == 0 && start == digits_start {
        rc = -1;
    } else if bytes[i..].iter().any(|byte| !is_space(*byte)) {
        rc = 1;
    }
    if digit_count < 19 {
        return (rc, value);
    }
    let order = if digit_count > 19 {
        std::cmp::Ordering::Greater
    } else {
        bytes[digits_start..i].cmp(b"9223372036854775808".as_slice())
    };
    match order {
        std::cmp::Ordering::Less => (rc, value),
        std::cmp::Ordering::Greater => (2, if neg { i64::MIN } else { i64::MAX }),
        std::cmp::Ordering::Equal => {
            if neg {
                (rc, i64::MIN)
            } else {
                (3, i64::MAX)
            }
        }
    }
}

/// `sqlite3RealToI64`.
fn real_to_i64(r: f64) -> i64 {
    if r < -9_223_372_036_854_774_784.0 {
        i64::MIN
    } else if r > 9_223_372_036_854_774_784.0 {
        i64::MAX
    } else {
        r as i64
    }
}

/// `sqlite3RealSameAsInt`: `r` is exactly the (small) integer `i`.
fn real_same_as_int(r: f64, i: i64) -> bool {
    r == 0.0
        || (r.to_bits() == (i as f64).to_bits()
            && (-2_251_799_813_685_248..2_251_799_813_685_248).contains(&i))
}

/// `computeNumericType`: the number a TEXT or BLOB operand of arithmetic
/// stands for. Always INTEGER or REAL.
pub(crate) fn arith_operand(bytes: &[u8]) -> SqlValue {
    let parsed = atof(bytes);
    if parsed.rc <= 0 {
        if parsed.rc == 0 {
            let (rc, ix) = atoi64(bytes);
            if rc <= 1 {
                return SqlValue::Integer(ix);
            }
        }
        return SqlValue::Real(parsed.value);
    }
    if parsed.rc == 1 {
        let (rc, ix) = atoi64(bytes);
        if rc == 0 {
            return SqlValue::Integer(ix);
        }
    }
    SqlValue::Real(parsed.value)
}

/// `sqlite3VdbeMemNumerify`: `CAST(x AS NUMERIC)` of a TEXT or BLOB.
pub(crate) fn numerify(bytes: &[u8]) -> SqlValue {
    let parsed = atof(bytes);
    if parsed.rc == 0 || parsed.rc == 1 {
        let (rc, ix) = atoi64(bytes);
        if rc <= 1 {
            return SqlValue::Integer(ix);
        }
    }
    let ix = real_to_i64(parsed.value);
    if real_same_as_int(parsed.value, ix) {
        SqlValue::Integer(ix)
    } else {
        SqlValue::Real(parsed.value)
    }
}

/// `applyNumericAffinity(p, 0)`: the number a TEXT stands for when the
/// whole text (spaces aside) is a well-formed number, else `None` and the
/// value stays TEXT.
pub(crate) fn comparison_number(text: &str) -> Option<SqlValue> {
    let bytes = text.as_bytes();
    let parsed = atof(bytes);
    if parsed.rc <= 0 {
        return None;
    }
    if parsed.rc == 1 {
        let ix = real_to_i64(parsed.value);
        if real_same_as_int(parsed.value, ix) {
            return Some(SqlValue::Integer(ix));
        }
        let (rc, ix) = atoi64(bytes);
        if rc == 0 {
            return Some(SqlValue::Integer(ix));
        }
    }
    Some(SqlValue::Real(parsed.value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(v: i64) -> SqlValue {
        SqlValue::Integer(v)
    }

    fn real(v: f64) -> SqlValue {
        SqlValue::Real(v)
    }

    #[test]
    fn arith_operand_matches_compute_numeric_type() {
        let cases: &[(&str, SqlValue)] = &[
            ("1", int(1)),
            (" 12 ", int(12)),
            ("12abc", int(12)),
            ("abc", int(0)),
            ("", int(0)),
            ("-", int(0)),
            (".", int(0)),
            ("1.0", real(1.0)),
            ("1.", real(1.0)),
            (".5", real(0.5)),
            ("1e2", real(100.0)),
            ("1e", int(1)),
            ("1e+", int(1)),
            ("1ex", int(1)),
            ("1e5x", real(100_000.0)),
            ("1.5e", real(1.5)),
            ("1.x", real(1.0)),
            ("inf", int(0)),
            ("-inf", int(0)),
            ("nan", int(0)),
            ("Infinity", int(0)),
            ("0x10", int(0)),
            ("-0", int(0)),
            ("\u{a0}5", int(0)),
            ("\x0b7", int(7)),
            ("9223372036854775807", int(i64::MAX)),
            ("-9223372036854775808", int(i64::MIN)),
            ("9223372036854775808", real(9_223_372_036_854_775_808.0)),
            ("-9223372036854775809", real(-9_223_372_036_854_775_809.0)),
            ("99999999999999999999abc", real(1e20)),
            ("1e400", real(f64::INFINITY)),
            ("2025-01-02", int(2025)),
        ];
        for (input, want) in cases {
            assert_eq!(arith_operand(input.as_bytes()), *want, "{input:?}");
        }
    }

    #[test]
    fn numerify_matches_sqlite_cast_to_numeric() {
        let cases: &[(&str, SqlValue)] = &[
            ("1.0", int(1)),
            ("1.5", real(1.5)),
            ("1e2", int(100)),
            ("1e5x", int(100_000)),
            ("1.x", int(1)),
            ("12abc", int(12)),
            ("inf", int(0)),
            ("nan", int(0)),
            ("-0.0", int(0)),
            ("2025-01-02", int(2025)),
            ("9223372036854775808", real(9_223_372_036_854_775_808.0)),
            ("1e400", real(f64::INFINITY)),
        ];
        for (input, want) in cases {
            assert_eq!(numerify(input.as_bytes()), *want, "{input:?}");
        }
    }

    #[test]
    fn comparison_number_converts_only_well_formed_numbers() {
        assert_eq!(comparison_number("5"), Some(int(5)));
        assert_eq!(comparison_number(" 5 "), Some(int(5)));
        assert_eq!(comparison_number("5.0"), Some(real(5.0)));
        assert_eq!(comparison_number("1e2"), Some(real(100.0)));
        assert_eq!(comparison_number("+.5"), Some(real(0.5)));
        assert_eq!(
            comparison_number("9223372036854775807"),
            Some(int(i64::MAX))
        );
        assert_eq!(
            comparison_number("9223372036854775808"),
            Some(real(9_223_372_036_854_775_808.0))
        );
        for text in [
            "5x", "abc", "", " ", "1e", "0x10", "inf", "nan", "\u{a0}5", "1.5e",
        ] {
            assert_eq!(comparison_number(text), None, "{text:?}");
        }
    }
}
