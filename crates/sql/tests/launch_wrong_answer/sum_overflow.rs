//! S9-06: `sum()` must raise `integer overflow` on every execution route.
//!
//! SQLite's `sumStep` adds integers with an overflow check; once the
//! running integer sum overflows, `sumFinalize` raises `integer overflow`
//! unless a non-integer value arrived afterwards. `total()` and `avg()`
//! share the accumulator but always answer with a REAL. RedlineDB has a
//! materialised aggregate path, a one-pass hash GROUP BY path and a window
//! path; all of them must give SQLite's answer.

use crate::lab::{Lab, int, real};

const MAX: &str = "9223372036854775807";

/// Routes over one table `t(g, x)` that must all agree with SQLite.
const ROUTES: &[&str] = &[
    "SELECT sum(x) FROM {t}",
    "SELECT sum(x), count(*) FROM {t}",
    "SELECT sum(DISTINCT x) FROM {t}",
    "SELECT sum(x) FILTER (WHERE x > 0) FROM {t}",
    "SELECT sum(x + 0) FROM {t}",
    "SELECT sum(x) + 1 FROM {t}",
    "SELECT sum(x + 1) FROM {t}",
    "SELECT g, sum(x) FROM {t} GROUP BY g",
    "SELECT g, sum(x), count(*) FROM {t} GROUP BY g",
    "SELECT g, sum(x) FROM {t} GROUP BY g HAVING count(*) > 0",
    "SELECT sum(x) OVER () FROM {t}",
    "SELECT sum(x) OVER (PARTITION BY g) FROM {t}",
    "SELECT sum(x) OVER (ORDER BY rowid) FROM {t}",
    "SELECT sum(x) OVER (ORDER BY rowid ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) FROM {t}",
    "SELECT sum(x) OVER (ORDER BY rowid ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING) FROM {t}",
    "SELECT total(x), avg(x) FROM {t}",
    "SELECT g, total(x), avg(x) FROM {t} GROUP BY g",
    "SELECT total(x) OVER (), avg(x) OVER () FROM {t}",
];

fn check_routes(lab: &Lab, table: &str) {
    for route in ROUTES {
        lab.assert_same(&route.replace("{t}", table), false);
    }
}

/// Routes for TEXT inputs: `x + 0` / `x + 1` on TEXT is the separate
/// text-arithmetic defect (launch Q5-05), so only direct aggregates run.
fn check_routes_without_text_arithmetic(lab: &Lab, table: &str) {
    for route in ROUTES.iter().filter(|route| !route.contains("x + ")) {
        lab.assert_same(&route.replace("{t}", table), false);
    }
}

fn load(lab: &Lab, table: &str, values: &[&str]) {
    lab.exec_both(&format!("CREATE TABLE {table}(g, x);"));
    for (i, value) in values.iter().enumerate() {
        lab.exec_both(&format!("INSERT INTO {table} VALUES ({}, {value});", i % 2));
    }
}

#[test]
fn sum_overflow_errors_on_every_route() {
    let lab = Lab::new();
    load(&lab, "two", &[MAX, "1"]);
    lab.assert_error("SELECT sum(x) FROM two", "integer overflow");
    lab.assert_error("SELECT sum(x + 0) FROM two", "integer overflow");
    lab.assert_error("SELECT sum(x) OVER () FROM two", "integer overflow");
    check_routes(&lab, "two");
    // Same group, so the GROUP BY answer overflows as well.
    lab.exec_both(&format!(
        "CREATE TABLE same(g, x); INSERT INTO same VALUES (0, {MAX}), (0, 1);"
    ));
    lab.assert_error("SELECT g, sum(x) FROM same GROUP BY g", "integer overflow");
    check_routes(&lab, "same");
}

#[test]
fn sum_overflow_cancellation_still_errors() {
    let lab = Lab::new();
    // MAX + 1 overflows before the -1 arrives; SQLite does not take the
    // overflow back.
    load(&lab, "cancel", &[MAX, "1", "-1"]);
    lab.exec_both("UPDATE cancel SET g = 0;");
    lab.assert_error("SELECT sum(x) FROM cancel", "integer overflow");
    check_routes(&lab, "cancel");
    // The other order never overflows and stays INTEGER.
    lab.exec_both(&format!(
        "CREATE TABLE fine(g, x); INSERT INTO fine VALUES (0, -1), (0, {MAX}), (0, 1);"
    ));
    lab.assert_rows("SELECT sum(x) FROM fine", &[vec![int(i64::MAX)]]);
    check_routes(&lab, "fine");
}

#[test]
fn sum_overflow_on_large_group_by() {
    // More than 5,000 rows: two groups, each holding MAX, 1 and -3, plus
    // zero rows. This is the one-pass hash GROUP BY route.
    let lab = Lab::new();
    lab.exec_both("CREATE TABLE big(g, x);");
    let mut values = Vec::with_capacity(5_006);
    for g in 0..2 {
        values.push(format!("({g}, {MAX})"));
        values.push(format!("({g}, 1)"));
        values.push(format!("({g}, -3)"));
    }
    for i in 0..5_000 {
        values.push(format!("({}, 0)", i % 2));
    }
    lab.exec_both(&format!("INSERT INTO big VALUES {};", values.join(", ")));
    lab.assert_error("SELECT g, sum(x) FROM big GROUP BY g", "integer overflow");
    lab.assert_error("SELECT sum(x) FROM big", "integer overflow");
    check_routes(&lab, "big");
}

#[test]
fn sum_with_a_real_value_stays_approximate() {
    let lab = Lab::new();
    // A REAL before the overflow: the sum is approximate from the start.
    load(&lab, "real_first", &["1.5", MAX, "1"]);
    lab.exec_both("UPDATE real_first SET g = 0;");
    check_routes(&lab, "real_first");
    // A REAL after the overflow clears it, as in SQLite's sumStep.
    load(&lab, "real_after", &[MAX, "1", "0.5"]);
    lab.exec_both("UPDATE real_after SET g = 0;");
    check_routes(&lab, "real_after");
    // Integers past 2^53 must not lose precision while they fit in i64.
    load(&lab, "wide", &["9007199254740992", "1", "1"]);
    lab.exec_both("UPDATE wide SET g = 0;");
    lab.assert_rows(
        "SELECT sum(x), total(x) FROM wide",
        &[vec![
            int(9_007_199_254_740_994),
            real(9_007_199_254_740_994.0),
        ]],
    );
    check_routes(&lab, "wide");
    // Numeric text and non-numeric text follow sqlite3_value_numeric_type.
    load(
        &lab,
        "texty",
        &["'5'", "' 6 '", "'2.5'", "'7abc'", "'abc'", "x'33'"],
    );
    check_routes_without_text_arithmetic(&lab, "texty");
    // Neumaier-compensated REAL sums.
    load(
        &lab,
        "tenths",
        &["0.1", "0.2", "0.3", "1e100", "1.0", "-1e100"],
    );
    check_routes(&lab, "tenths");
}

#[test]
fn sum_overflow_in_group_by_error_stops_the_statement() {
    let lab = Lab::new();
    lab.exec_both(&format!(
        "CREATE TABLE mix(g, x); INSERT INTO mix VALUES (0, 1), (0, 2), (1, {MAX}), (1, 1);"
    ));
    // SQLite reports the error for the whole statement, not a partial set
    // of groups.
    lab.assert_error(
        "SELECT g, sum(x) FROM mix GROUP BY g ORDER BY g",
        "integer overflow",
    );
    lab.assert_rows(
        "SELECT g, sum(x) FROM mix WHERE g = 0 GROUP BY g",
        &[vec![int(0), int(3)]],
    );
}

/// Sliding frames: SQLite keeps one accumulator per partition, adding rows
/// as they enter the frame and removing them as they leave, so a REAL or an
/// overflow that passed through the frame still shows in later rows.
const FRAMES: &[&str] = &[
    "ROWS BETWEEN CURRENT ROW AND CURRENT ROW",
    "ROWS BETWEEN 1 PRECEDING AND CURRENT ROW",
    "ROWS BETWEEN 2 PRECEDING AND CURRENT ROW",
    "ROWS BETWEEN CURRENT ROW AND 1 FOLLOWING",
    "ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING",
    "ROWS BETWEEN 1 PRECEDING AND 1 PRECEDING",
    "ROWS BETWEEN 2 PRECEDING AND 1 PRECEDING",
    "ROWS BETWEEN 1 FOLLOWING AND 1 FOLLOWING",
    "ROWS BETWEEN 1 FOLLOWING AND 2 FOLLOWING",
    "ROWS BETWEEN 2 FOLLOWING AND 3 FOLLOWING",
    "ROWS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING",
    "ROWS BETWEEN 1 PRECEDING AND UNBOUNDED FOLLOWING",
    "ROWS BETWEEN 1 FOLLOWING AND UNBOUNDED FOLLOWING",
    "ROWS BETWEEN UNBOUNDED PRECEDING AND 1 FOLLOWING",
];

#[test]
fn window_sum_follows_sqlite_accumulator() {
    let lab = Lab::new();
    let tables: &[(&str, &[&str])] = &[
        ("w_over", &[MAX, "1", "3", "4"]),
        ("w_cancel", &["1", MAX, "-9223372036854775807", "4", "5"]),
        ("w_real", &["1.5", "2", "3", "4", "5"]),
        ("w_real_max", &["1.5", MAX, "1", "2"]),
        ("w_ints", &["1", "2", "3", "4", "5", "6"]),
        (
            "w_tenths",
            &["0.1", "0.2", "0.3", "1e100", "1.0", "-1e100", "7"],
        ),
    ];
    for (table, values) in tables {
        lab.exec_both(&format!("CREATE TABLE {table}(x);"));
        for value in *values {
            lab.exec_both(&format!("INSERT INTO {table} VALUES ({value});"));
        }
        for frame in FRAMES {
            for func in ["sum(x)", "total(x)", "avg(x)", "count(x)", "first_value(x)"] {
                lab.assert_same(
                    &format!(
                        "SELECT {func} OVER (ORDER BY rowid {frame}) FROM {table} ORDER BY rowid"
                    ),
                    true,
                );
            }
        }
    }
    lab.assert_rows(
        "SELECT sum(x) OVER (ORDER BY rowid ROWS BETWEEN 1 PRECEDING AND 1 PRECEDING) \
         FROM w_ints ORDER BY rowid LIMIT 2",
        &[vec![crate::lab::null()], vec![int(1)]],
    );
}
