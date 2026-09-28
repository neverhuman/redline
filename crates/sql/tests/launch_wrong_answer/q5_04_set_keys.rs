//! Q5-04: UNION, INTERSECT, EXCEPT and recursive UNION compare rows the way
//! SQLite compares values.
//!
//! The compound set operations keyed rows by an unescaped string
//! (`T<text>|` per TEXT value), so `('a|Tb', 'c')` and `('a', 'b|Tc')`
//! collided, and by storage class, so INTEGER 1 and REAL 1.0 stayed two
//! rows. The recursive-CTE UNION keyed rows by record bytes, which also
//! keep 1 and 1.0 apart. SQLite keeps the last of equal rows for UNION
//! (`1 UNION 1.0` is REAL 1.0), the left row for INTERSECT and EXCEPT, and
//! the first row for a recursive UNION.

use redlinedb_sql::SqlValue;

use crate::lab::{Lab, int, real, text};

fn count(lab: &Lab, compound: &str, want: i64) {
    lab.assert_rows(
        &format!("SELECT count(*) FROM ({compound})"),
        &[vec![int(want)]],
    );
}

#[test]
fn q5_04_delimiter_collision_union() {
    let lab = Lab::new();
    count(&lab, "SELECT 'a|Tb', 'c' UNION SELECT 'a', 'b|Tc'", 2);
    lab.assert_same("SELECT 'a|Tb', 'c' UNION SELECT 'a', 'b|Tc'", false);
}

#[test]
fn q5_04_delimiter_collision_intersect() {
    let lab = Lab::new();
    count(&lab, "SELECT 'a|Tb', 'c' INTERSECT SELECT 'a', 'b|Tc'", 0);
}

#[test]
fn q5_04_delimiter_collision_except() {
    let lab = Lab::new();
    count(&lab, "SELECT 'a|Tb', 'c' EXCEPT SELECT 'a', 'b|Tc'", 1);
    lab.assert_rows(
        "SELECT 'a|Tb', 'c' EXCEPT SELECT 'a', 'b|Tc'",
        &[vec![text("a|Tb"), text("c")]],
    );
}

#[test]
fn q5_04_int_real_union_typed() {
    let lab = Lab::new();
    count(&lab, "SELECT 1 UNION SELECT 1.0", 1);
    lab.assert_rows("SELECT 1 UNION SELECT 1.0", &[vec![real(1.0)]]);
    lab.assert_rows("SELECT 1.0 UNION SELECT 1", &[vec![int(1)]]);
    lab.assert_rows(
        "SELECT x, typeof(x) FROM (SELECT 1 AS x UNION SELECT 1.0)",
        &[vec![real(1.0), text("real")]],
    );
    lab.assert_rows("SELECT 1 UNION SELECT 1.0 UNION SELECT 1", &[vec![int(1)]]);
    lab.assert_rows(
        "SELECT 1 AS x UNION SELECT 1.0 ORDER BY 1",
        &[vec![real(1.0)]],
    );
    lab.assert_rows("SELECT 1.0 AS x UNION SELECT 1 ORDER BY 1", &[vec![int(1)]]);
    lab.assert_same(
        "SELECT 1 UNION SELECT 2.0 UNION SELECT 2 UNION SELECT 1.0",
        false,
    );
}

#[test]
fn q5_04_int_real_intersect() {
    let lab = Lab::new();
    count(&lab, "SELECT 1 INTERSECT SELECT 1.0", 1);
    lab.assert_rows("SELECT 1 INTERSECT SELECT 1.0", &[vec![int(1)]]);
    lab.assert_rows("SELECT 1.0 INTERSECT SELECT 1", &[vec![real(1.0)]]);
    lab.assert_rows(
        "SELECT 1.0 INTERSECT SELECT 1 INTERSECT SELECT 1.0",
        &[vec![real(1.0)]],
    );
    lab.assert_rows(
        "SELECT 1 AS x INTERSECT SELECT 1.0 ORDER BY 1",
        &[vec![int(1)]],
    );
}

#[test]
fn q5_04_int_real_except() {
    let lab = Lab::new();
    count(&lab, "SELECT 1 EXCEPT SELECT 1.0", 0);
    count(&lab, "SELECT 1.0 EXCEPT SELECT 1", 0);
    lab.assert_rows(
        "SELECT 1 AS x UNION ALL SELECT 2.0 EXCEPT SELECT 2",
        &[vec![int(1)]],
    );
}

#[test]
fn q5_04_multi_column() {
    let lab = Lab::new();
    count(&lab, "SELECT 1, 'x' UNION SELECT 1.0, 'x'", 1);
    lab.assert_rows(
        "SELECT 1, 'x' UNION SELECT 1.0, 'x'",
        &[vec![real(1.0), text("x")]],
    );
    count(&lab, "SELECT 1, 'x' INTERSECT SELECT 1.0, 'x'", 1);
    count(&lab, "SELECT 1, 'x' EXCEPT SELECT 1.0, 'x'", 0);
    count(&lab, "SELECT 1, 'x' UNION SELECT 1.0, 'y'", 2);
}

#[test]
fn q5_04_recursive_union_int_real() {
    let lab = Lab::new();
    lab.assert_rows(
        "WITH RECURSIVE r(x) AS (SELECT 1 UNION SELECT 1.0 FROM r) SELECT count(*) FROM r",
        &[vec![int(1)]],
    );
    // The recursive UNION keeps the row it saw first.
    lab.assert_rows(
        "WITH RECURSIVE r(x) AS (SELECT 1 UNION SELECT 1.0 FROM r) SELECT x, typeof(x) FROM r",
        &[vec![int(1), text("integer")]],
    );
    lab.assert_rows(
        "WITH RECURSIVE r(x) AS (SELECT 2.0 UNION \
         SELECT CASE WHEN x > 0 THEN x - 1 ELSE 2 END FROM r) \
         SELECT x, typeof(x) FROM r ORDER BY x",
        &[
            vec![real(0.0), text("real")],
            vec![real(1.0), text("real")],
            vec![real(2.0), text("real")],
        ],
    );
    lab.assert_rows(
        "WITH RECURSIVE r(a, b) AS (SELECT 'a|Tb', 'c' UNION SELECT 'a', 'b|Tc' FROM r) \
         SELECT count(*) FROM r",
        &[vec![int(2)]],
    );
}

#[test]
fn q5_04_nul_and_delimiter_bytes() {
    let lab = Lab::new();
    count(&lab, "SELECT x'610062' UNION SELECT x'61'", 2);
    count(&lab, "SELECT x'00' UNION SELECT ''", 2);
    count(&lab, "SELECT 'a' UNION SELECT x'61'", 2);
    count(&lab, "SELECT '1' UNION SELECT 1", 2);
    count(&lab, "SELECT 'N|' UNION SELECT NULL", 2);
    count(&lab, "SELECT NULL UNION SELECT NULL", 1);
    count(&lab, "SELECT 0 UNION SELECT 0.0 UNION SELECT -0.0", 1);
    count(&lab, "SELECT 'N|', NULL UNION SELECT NULL, 'N|'", 2);
}

#[test]
fn q5_04_exact_integer_real_equality() {
    let lab = Lab::new();
    count(
        &lab,
        "SELECT 9007199254740993 UNION SELECT 9007199254740992.0",
        2,
    );
    count(
        &lab,
        "SELECT 9007199254740992 UNION SELECT 9007199254740992.0",
        1,
    );
    count(
        &lab,
        "SELECT 9223372036854775807 UNION SELECT 9223372036854775807.0",
        2,
    );
    count(
        &lab,
        "SELECT -9223372036854775808 UNION SELECT -9223372036854775808.0",
        1,
    );
    count(&lab, "SELECT 1.5 UNION SELECT 1 UNION SELECT 2", 3);
    count(
        &lab,
        "SELECT 1e300 UNION SELECT 1e300 * 10 UNION SELECT 1e308 * 10",
        3,
    );
    count(&lab, "SELECT 1e308 * 10 UNION SELECT 1e309", 1);
}

#[test]
fn q5_04_stored_values_and_bound_parameters() {
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE t(x); INSERT INTO t VALUES (1), (1.0), (2), ('1'), (x'31'), (NULL);",
    );
    count(&lab, "SELECT x FROM t UNION SELECT x FROM t", 5);
    count(&lab, "SELECT x FROM t INTERSECT SELECT 2.0", 1);
    count(&lab, "SELECT x FROM t EXCEPT SELECT 1.0", 4);
    lab.assert_same("SELECT x FROM t EXCEPT SELECT 1.0", false);
    let params = [int(1), real(1.0)];
    let out = lab.assert_same_bound("SELECT ?1 UNION SELECT ?2", &params, true);
    assert!(
        matches!(out, crate::lab::Outcome::Rows(ref rows) if rows == &vec![vec![SqlValue::Real(1.0)]]),
        "sqlite answered {out:?}"
    );
    lab.assert_same_bound("SELECT ?1 INTERSECT SELECT ?2", &params, true);
    lab.assert_same_bound("SELECT ?1 EXCEPT SELECT ?2", &params, true);
}

/// Equal rows inside one operand. SQLite 3.53.1 (the pinned parity
/// reference) keeps the first of them and lets the right operand win a tie
/// across operands; bundled SQLite 3.50 (the lab's oracle) keeps the last
/// row inserted into its temporary index instead, so these answers are
/// pinned against 3.53.1 output rather than compared with the lab.
#[test]
fn q5_04_equal_rows_inside_one_operand_follow_sqlite_3_53() {
    let lab = Lab::new();
    lab.exec_both("CREATE TABLE t(x); INSERT INTO t VALUES (1.0), (1);");
    let cases: [(&str, Vec<Vec<SqlValue>>); 4] = [
        (
            "SELECT x, typeof(x) FROM (SELECT x FROM t UNION SELECT 2)",
            vec![vec![real(1.0), text("real")], vec![int(2), text("integer")]],
        ),
        (
            "SELECT x, typeof(x) FROM (SELECT x FROM t INTERSECT SELECT 1)",
            vec![vec![real(1.0), text("real")]],
        ),
        (
            "SELECT x, typeof(x) FROM (SELECT 1 AS x UNION ALL SELECT 1.0 EXCEPT SELECT 2)",
            vec![vec![int(1), text("integer")]],
        ),
        (
            "SELECT x, typeof(x) FROM (SELECT 1 AS x UNION ALL SELECT 1.0 UNION SELECT 2)",
            vec![vec![int(1), text("integer")], vec![int(2), text("integer")]],
        ),
    ];
    for (sql, want) in cases {
        match lab.rows_redline(sql, &[]) {
            crate::lab::Outcome::Rows(mut rows) => {
                rows.sort_by_key(|row| format!("{row:?}"));
                let mut want = want;
                want.sort_by_key(|row| format!("{row:?}"));
                assert_eq!(rows, want, "`{sql}`");
            }
            crate::lab::Outcome::Err(err) => panic!("`{sql}` failed: {err}"),
        }
    }
}
