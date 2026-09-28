//! SQLite numeric primitives shared by the kernel evaluator and the SQL
//! layer: the exact INTEGER/REAL comparison and the numeric-prefix reading
//! of TEXT and BLOB that SQLite uses for truth values and `sum()` inputs.

use std::cmp::Ordering;

/// 2^63 as an `f64`; `i64::MAX as f64` rounds up to this value.
const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;

/// Compare an INTEGER with a REAL exactly, as SQLite's
/// `sqlite3IntFloatCompare` does. Casting the integer to `f64` first is
/// lossy above 2^53: `9007199254740993` would compare equal to
/// `9007199254740992.0`. A NaN is treated as NULL, which every integer
/// exceeds.
pub fn int_real_cmp(i: i64, r: f64) -> Ordering {
    if r.is_nan() {
        return Ordering::Greater;
    }
    if r < -TWO_POW_63 {
        return Ordering::Greater;
    }
    if r >= TWO_POW_63 {
        return Ordering::Less;
    }
    // `r` is inside [-2^63, 2^63), so truncation toward zero is exact in i64.
    let y = r as i64;
    match i.cmp(&y) {
        Ordering::Equal => {
            // Same integer part; the fraction of `r` decides. `i` equals the
            // truncation of `r`, so `i as f64` is exact whenever `r` has a
            // fractional part (|r| < 2^52 there).
            (i as f64).partial_cmp(&r).unwrap_or(Ordering::Equal)
        }
        other => other,
    }
}

/// The value of the longest numeric prefix of `bytes`, the way SQLite's
/// `sqlite3AtoF` reads a TEXT or BLOB that is not a well-formed number:
/// optional leading whitespace, an optional sign, digits with an optional
/// `.` and fraction, and an exponent only when digits follow the `e`.
/// Reading stops at the first byte that does not fit; no digits gives 0.0.
/// So `'1abc'` is 1.0, `'.5x'` is 0.5, `'1e'` is 1.0 and `'abc'` is 0.0.
pub fn sqlite_numeric_prefix(bytes: &[u8]) -> f64 {
    let mut i = 0;
    while i < bytes.len() && is_sqlite_space(bytes[i]) {
        i += 1;
    }
    let start = i;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    let int_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let mut digits = i - int_start;
    if i < bytes.len() && bytes[i] == b'.' {
        let frac_start = i + 1;
        let mut j = frac_start;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        digits += j - frac_start;
        i = j;
    }
    if digits == 0 {
        return 0.0;
    }
    let mut end = i;
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        let mut j = i + 1;
        if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
            j += 1;
        }
        let exp_start = j;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_start {
            end = j;
        }
    }
    // The prefix is ASCII by construction and matches Rust's float grammar
    // (a trailing or leading `.` next to digits included).
    std::str::from_utf8(&bytes[start..end])
        .ok()
        .and_then(|text| text.parse::<f64>().ok())
        .unwrap_or(0.0)
}

/// SQLite's truth value of a TEXT or BLOB: its numeric prefix is non-zero.
pub fn sqlite_text_is_true(bytes: &[u8]) -> bool {
    sqlite_numeric_prefix(bytes) != 0.0
}

/// `sqlite3Isspace`: space, tab, newline, vertical tab, form feed, CR.
fn is_sqlite_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_real_cmp_is_exact_above_2p53() {
        assert_eq!(
            int_real_cmp(9_007_199_254_740_993, 9_007_199_254_740_992.0),
            Ordering::Greater
        );
        assert_eq!(
            int_real_cmp(9_007_199_254_740_992, 9_007_199_254_740_992.0),
            Ordering::Equal
        );
        assert_eq!(int_real_cmp(i64::MAX, TWO_POW_63), Ordering::Less);
        assert_eq!(int_real_cmp(i64::MIN, -TWO_POW_63), Ordering::Equal);
        assert_eq!(int_real_cmp(i64::MIN, -1e19), Ordering::Greater);
        assert_eq!(int_real_cmp(1, 1.5), Ordering::Less);
        assert_eq!(int_real_cmp(-1, -1.5), Ordering::Greater);
        assert_eq!(int_real_cmp(0, f64::NAN), Ordering::Greater);
    }

    #[test]
    fn numeric_prefix_matches_sqlite_atof() {
        let cases: &[(&[u8], f64)] = &[
            (b"1abc", 1.0),
            (b"\x31\xff", 1.0),
            (b"1e", 1.0),
            (b"1e+", 1.0),
            (b"2e3x", 2000.0),
            (b".5x", 0.5),
            (b"5.", 5.0),
            (b"  -3.5z", -3.5),
            (b"+7", 7.0),
            (b"abc", 0.0),
            (b"", 0.0),
            (b".", 0.0),
            (b"-", 0.0),
            (b"inf", 0.0),
            (b"nan", 0.0),
            (b"0x10", 0.0),
            (b"\t\n 4", 4.0),
        ];
        for (input, want) in cases {
            assert_eq!(sqlite_numeric_prefix(input), *want, "{input:?}");
        }
        assert!(sqlite_numeric_prefix(b"1e400").is_infinite());
    }
}
