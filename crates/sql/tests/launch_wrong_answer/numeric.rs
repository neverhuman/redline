//! Q5-06: integer arithmetic that overflows, INTEGER/REAL comparison above
//! 2^53, and the numeric-prefix truthiness SQLite applies to TEXT and BLOB.
//!
//! SQLite promotes an overflowing `+`, `-`, `*`, unary minus and
//! `MIN / -1` to REAL, answers `MIN % -1` with 0, and raises
//! `integer overflow` for `abs(MIN)`. It compares an INTEGER with a REAL
//! exactly (`sqlite3IntFloatCompare`), and it reads a TEXT or BLOB as a
//! boolean through its longest numeric prefix, so `'1abc'` is true.

use crate::lab::{Lab, int, real, text};

#[path = "numeric/truthiness.rs"]
mod truthiness;

const MAX: &str = "9223372036854775807";
const MIN: &str = "(-9223372036854775807-1)";

/// (left, op, right) pairs whose integer result does not fit in i64, plus
/// the division and remainder edge cases and ordinary controls.
const OVERFLOW_CASES: &[(&str, &str, &str)] = &[
    (MAX, "+", "1"),
    (MAX, "-", "-1"),
    (MIN, "-", "1"),
    (MIN, "+", "-1"),
    (MAX, "*", "2"),
    (MIN, "*", "-1"),
    ("4294967296", "*", "4294967296"),
    ("-4294967296", "*", "4294967296"),
    (MIN, "/", "-1"),
    (MIN, "%", "-1"),
    (MAX, "/", "-1"),
    (MAX, "%", "-1"),
    ("7", "/", "0"),
    ("7", "%", "0"),
    ("-7", "/", "2"),
    ("-7", "%", "3"),
    ("5.5", "%", "2"),
    ("7", "%", "2.5"),
    ("7", "%", "0.5"),
    (MAX, "+", "0"),
];

#[test]
fn overflow_promotes_to_real() {
    let lab = Lab::new();
    lab.exec_both("CREATE TABLE ov(a, b); CREATE TABLE ovi(a INTEGER, b INTEGER);");
    for (left, op, right) in OVERFLOW_CASES {
        // Literal form.
        lab.assert_same(&format!("SELECT {left} {op} {right}"), true);
        lab.assert_same(&format!("SELECT typeof({left} {op} {right})"), true);
        // FROM-subquery form reaches the engine's row evaluator.
        lab.assert_same(
            &format!("SELECT a {op} b, typeof(a {op} b) FROM (SELECT {left} AS a, {right} AS b)"),
            true,
        );
        // Stored columns reach the table-scan evaluator and WHERE.
        for table in ["ov", "ovi"] {
            lab.exec_both(&format!(
                "DELETE FROM {table}; INSERT INTO {table} VALUES ({left}, {right});"
            ));
            lab.assert_same(
                &format!("SELECT a {op} b, typeof(a {op} b) FROM {table}"),
                true,
            );
            lab.assert_same(
                &format!("SELECT count(*) FROM {table} WHERE a {op} b > 0"),
                true,
            );
            // Aggregate context evaluates the arithmetic after grouping.
            lab.assert_same(&format!("SELECT max(a) {op} max(b) FROM {table}"), true);
        }
    }
}

#[test]
fn overflow_promotes_to_real_with_bound_parameters() {
    // Bound parameters inside a FROM-subquery read as NULL today (a
    // separate derived-table binding defect), so the column forms bind
    // against a stored value instead.
    let lab = Lab::new();
    lab.exec_both(&format!(
        "CREATE TABLE bp(big INTEGER, small INTEGER); \
         INSERT INTO bp VALUES (9223372036854775807, {MIN});"
    ));
    let max = int(i64::MAX);
    let min = int(i64::MIN);
    let cases = [
        (max.clone(), "+", int(1)),
        (max.clone(), "*", int(2)),
        (min.clone(), "-", int(1)),
        (min.clone(), "/", int(-1)),
        (min.clone(), "%", int(-1)),
        (min.clone(), "*", int(-1)),
    ];
    for (left, op, right) in cases {
        let params = [left, right.clone()];
        lab.assert_same_bound(
            &format!("SELECT ?1 {op} ?2, typeof(?1 {op} ?2)"),
            &params,
            true,
        );
        lab.assert_same_bound(
            &format!("SELECT big {op} ?1, small {op} ?1, typeof(big {op} ?1) FROM bp"),
            std::slice::from_ref(&right),
            true,
        );
        lab.assert_same_bound(
            &format!("SELECT count(*) FROM bp WHERE big {op} ?1 > 0"),
            std::slice::from_ref(&right),
            true,
        );
    }
    lab.assert_same_bound("SELECT -?1, typeof(-?1)", &[min.clone()], true);
    lab.assert_same_bound("SELECT count(*) FROM bp WHERE small = -?1", &[min], true);
}

#[test]
fn unary_minus_of_min_promotes_to_real() {
    let lab = Lab::new();
    lab.assert_same(&format!("SELECT -{MIN}, typeof(-{MIN})"), true);
    lab.assert_same(
        &format!("SELECT -a, typeof(-a) FROM (SELECT {MIN} AS a)"),
        true,
    );
    lab.exec_both(&format!(
        "CREATE TABLE n(a INTEGER); INSERT INTO n VALUES ({MIN});"
    ));
    lab.assert_same("SELECT -a, typeof(-a) FROM n", true);
    lab.assert_same("SELECT -max(a) FROM n", true);
    lab.assert_rows(
        &format!("SELECT -{MIN}"),
        &[vec![real(9_223_372_036_854_775_808.0)]],
    );
}

#[test]
fn abs_of_min_is_an_integer_overflow_error() {
    let lab = Lab::new();
    lab.assert_error(&format!("SELECT abs({MIN})"), "integer overflow");
    lab.assert_error(
        &format!("SELECT abs(a) FROM (SELECT {MIN} AS a)"),
        "integer overflow",
    );
    lab.exec_both(&format!(
        "CREATE TABLE n(a INTEGER); INSERT INTO n VALUES ({MIN});"
    ));
    lab.assert_error("SELECT abs(a) FROM n", "integer overflow");
    lab.assert_rows(&format!("SELECT abs({MIN} + 1)"), &[vec![int(i64::MAX)]]);
}

#[test]
fn int_real_compare_above_2p53() {
    let lab = Lab::new();
    for (l, r) in [
        ("9007199254740993", "9007199254740992.0"),
        ("9007199254740992", "9007199254740992.0"),
        ("-9007199254740993", "-9007199254740992.0"),
        (MAX, "9223372036854775807.0"),
        (MIN, "-9223372036854775808.0"),
        ("9223372036854775806", "9223372036854775807.0"),
    ] {
        let sql = format!("SELECT {l} = {r}, {l} > {r}, {l} < {r}, {r} < {l}, {r} = {l}");
        lab.assert_same(&sql, true);
        lab.assert_same(
            &format!("SELECT a = b, a > b, a < b, b < a FROM (SELECT {l} AS a, {r} AS b)"),
            true,
        );
    }
    lab.assert_rows(
        "SELECT 9007199254740993 = 9007199254740992.0, 9007199254740993 > 9007199254740992.0",
        &[vec![int(0), int(1)]],
    );
    lab.assert_same_bound(
        "SELECT ?1 = ?2, ?1 > ?2",
        &[int(9_007_199_254_740_993), real(9_007_199_254_740_992.0)],
        true,
    );

    lab.exec_both(
        "CREATE TABLE big(v); INSERT INTO big VALUES \
         (9007199254740993), (9007199254740992.5e0), (9007199254740994.0), \
         (9007199254740991), (9007199254740992);",
    );
    lab.assert_same("SELECT v, typeof(v) FROM big ORDER BY v", true);
    lab.assert_same("SELECT v, typeof(v) FROM big ORDER BY v DESC", true);
    lab.assert_same("SELECT min(v), max(v) FROM big", true);
    lab.assert_same(
        "SELECT v FROM big WHERE v > 9007199254740992.0 ORDER BY v",
        true,
    );
    lab.assert_same(
        "SELECT v FROM big WHERE v = 9007199254740992.0 ORDER BY v",
        true,
    );

    lab.exec_both(
        "CREATE TABLE edge(v); INSERT INTO edge VALUES \
         (9223372036854775807), (9223372036854775807.0), (9223372036854775806);",
    );
    lab.assert_same("SELECT v, typeof(v) FROM edge ORDER BY v", true);
    lab.assert_same("SELECT min(v), max(v), typeof(max(v)) FROM edge", true);
}

#[test]
fn update_delta_overflow() {
    let lab = Lab::new();
    lab.exec_both(&format!(
        "CREATE TABLE u(x INTEGER, y); INSERT INTO u VALUES ({MAX}, {MIN}); \
         CREATE TABLE k(id INTEGER PRIMARY KEY, x INTEGER); INSERT INTO k VALUES (1, {MAX});"
    ));
    lab.step_both("UPDATE u SET x = x + 1");
    lab.assert_same("SELECT x, typeof(x) FROM u", true);
    lab.step_both("UPDATE u SET y = y - 1");
    lab.assert_same("SELECT y, typeof(y) FROM u", true);
    lab.step_both("UPDATE k SET x = x + 1 WHERE id = 1");
    lab.assert_same("SELECT x, typeof(x) FROM k", true);
    lab.assert_rows(
        "SELECT x, typeof(x) FROM k",
        &[vec![real(9_223_372_036_854_775_808.0), text("real")]],
    );

    lab.exec_both(&format!("UPDATE u SET x = {MAX}, y = {MIN};"));
    lab.assert_same_bound("UPDATE u SET x = x + ?1", &[int(1)], true);
    lab.assert_same("SELECT x, typeof(x) FROM u", true);
    lab.assert_same_bound("UPDATE u SET y = y - ?1", &[int(1)], true);
    lab.assert_same("SELECT y, typeof(y) FROM u", true);
}
