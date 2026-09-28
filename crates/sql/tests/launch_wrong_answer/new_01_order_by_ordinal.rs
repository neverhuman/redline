//! NEW-01: `ORDER BY <n>` sorts by the n-th result column whatever its name.
//!
//! The binder rewrote `ORDER BY 2` into the second output column's name,
//! and only when that name was `[A-Za-z0-9_]+`. For `-x`, `x*10`,
//! `count(*)` or `upper(g)` it left the integer as a constant, so the
//! rows came back unsorted. SQLite also reads `+1`, `(1)`, `- -1`, `0x1`
//! and `1 COLLATE NOCASE` as positions, and rejects 0 and negative
//! positions with `1st ORDER BY term out of range`.

use crate::lab::{Lab, Outcome, int, text};

fn lab() -> Lab {
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE t(x INTEGER, g TEXT, y INTEGER); \
         INSERT INTO t VALUES (1, 'a', 50), (3, 'a', 40), (2, 'b', 30), (5, 'A', 20), (4, 'b', 10);",
    );
    lab
}

#[test]
fn new_01_order_by_ordinal_negated_column() {
    let lab = lab();
    lab.assert_rows(
        "SELECT -x FROM t ORDER BY 1",
        &[
            vec![int(-5)],
            vec![int(-4)],
            vec![int(-3)],
            vec![int(-2)],
            vec![int(-1)],
        ],
    );
}

#[test]
fn new_01_order_by_ordinal_expression_desc() {
    let lab = lab();
    lab.assert_rows(
        "SELECT x, x * 10 FROM t ORDER BY 2 DESC",
        &[
            vec![int(5), int(50)],
            vec![int(4), int(40)],
            vec![int(3), int(30)],
            vec![int(2), int(20)],
            vec![int(1), int(10)],
        ],
    );
}

#[test]
fn new_01_order_by_ordinal_aggregate() {
    let lab = lab();
    lab.assert_rows(
        "SELECT g, count(*) FROM t GROUP BY g ORDER BY 2, 1",
        &[
            vec![text("A"), int(1)],
            vec![text("a"), int(2)],
            vec![text("b"), int(2)],
        ],
    );
}

#[test]
fn new_01_order_by_ordinal_shapes() {
    let lab = lab();
    for sql in [
        "SELECT upper(g), x FROM t ORDER BY 1, 2 DESC",
        "SELECT x AS \"my col\" FROM t ORDER BY 1 DESC",
        "SELECT x + y, x FROM t ORDER BY 1, 2",
        "SELECT g, sum(y) FROM t GROUP BY g ORDER BY 2 DESC",
        "SELECT g, count(*) AS n FROM t GROUP BY g HAVING count(*) > 0 ORDER BY 2, 1 DESC",
        "SELECT DISTINCT -x FROM t ORDER BY 1",
        "SELECT * FROM t ORDER BY 3",
        "SELECT *, -x FROM t ORDER BY 4",
        "SELECT t.* FROM t ORDER BY 2, 1",
        "SELECT -x FROM t ORDER BY 1 LIMIT 2",
        "SELECT -x FROM t ORDER BY 1 LIMIT 2 OFFSET 1",
        "SELECT x, -x FROM t ORDER BY 2 DESC LIMIT 3",
        "SELECT g COLLATE NOCASE, x FROM t ORDER BY 1, 2",
        "SELECT g, x FROM t ORDER BY 1 COLLATE NOCASE, 2 DESC",
        "SELECT -x FROM t ORDER BY +1",
        "SELECT -x FROM t ORDER BY (1)",
        "SELECT -x FROM t ORDER BY - -1",
        "SELECT -x FROM t ORDER BY 1.0, x",
        "SELECT -x FROM t ORDER BY '1', x",
        "SELECT x, row_number() OVER (ORDER BY y) FROM t ORDER BY 2",
        "SELECT -x FROM t WHERE x > 1 ORDER BY 1",
        "SELECT (SELECT count(*) FROM t AS u WHERE u.x < t.x) FROM t ORDER BY 1 DESC",
        "SELECT CASE WHEN x > 2 THEN -x ELSE x END FROM t ORDER BY 1",
        "SELECT x AS z FROM t ORDER BY 1 DESC",
        "SELECT x AS z, g FROM t ORDER BY 2, 1",
        "SELECT DISTINCT g, x FROM t ORDER BY 1 COLLATE NOCASE, 2",
        "SELECT DISTINCT g COLLATE NOCASE, x FROM t ORDER BY 1, 2 DESC",
        "SELECT g, count(*) FROM t GROUP BY g ORDER BY 1 COLLATE NOCASE DESC, 2",
        "SELECT g, count(*) FROM t GROUP BY g ORDER BY g COLLATE NOCASE DESC, 2",
        "SELECT g || '', count(*) FROM t GROUP BY g ORDER BY 1 COLLATE NOCASE, 2 DESC",
        "SELECT g, sum(y) FROM t GROUP BY g ORDER BY 2",
        "SELECT g, sum(y) + 1 FROM t GROUP BY g ORDER BY 2 DESC",
        "SELECT g, max(x) AS m FROM t GROUP BY g HAVING max(x) > 1 ORDER BY 2",
        "SELECT (SELECT -x FROM t ORDER BY 1 LIMIT 1)",
        "SELECT * FROM t AS a JOIN t AS b ON a.x = 6 - b.x ORDER BY 4",
    ] {
        lab.assert_same(sql, true);
    }
}

/// A result column named like a source column must still sort by the
/// result column, not by the source column of that name.
#[test]
fn new_01_order_by_ordinal_shadowed_names() {
    let lab = lab();
    lab.assert_same("SELECT y AS x, x AS y FROM t ORDER BY 1", true);
    lab.assert_same("SELECT y AS x, x AS y FROM t ORDER BY 2 DESC", true);
    lab.assert_same("SELECT -x AS x FROM t ORDER BY 1", true);
    lab.assert_same(
        "SELECT a.x, b.x FROM t AS a JOIN t AS b ON a.x + b.x = 6 ORDER BY 2",
        true,
    );
}

#[test]
fn new_01_order_by_ordinal_compound_values_and_subquery() {
    let lab = lab();
    for sql in [
        "SELECT -x FROM t UNION ALL SELECT -10 ORDER BY 1",
        "SELECT -x FROM t UNION SELECT -10 ORDER BY 1 DESC",
        "SELECT x * 10 FROM t EXCEPT SELECT 20 ORDER BY 1",
        "SELECT * FROM (SELECT -x FROM t) ORDER BY 1",
        "SELECT * FROM (SELECT -x, g FROM t) ORDER BY 2, 1",
        "WITH c AS (SELECT -x AS v FROM t) SELECT v * 2 FROM c ORDER BY 1",
        "SELECT * FROM (VALUES (3), (1), (2)) ORDER BY 1",
    ] {
        lab.assert_same(sql, true);
    }
}

fn assert_out_of_range(lab: &Lab, sql: &str, message: &str) {
    for (engine, outcome) in [
        ("sqlite", lab.rows_sqlite(sql, &[])),
        ("redline", lab.rows_redline(sql, &[])),
    ] {
        match outcome {
            Outcome::Err(err) => assert!(
                err.contains(message),
                "{engine} failed `{sql}` with `{err}`, expected `{message}`"
            ),
            Outcome::Rows(rows) => panic!("{engine} answered {rows:?} for `{sql}`"),
        }
    }
}

#[test]
fn new_01_order_by_ordinal_out_of_range() {
    let lab = lab();
    let one = "1st ORDER BY term out of range - should be between 1 and 1";
    assert_out_of_range(&lab, "SELECT -x FROM t ORDER BY 2", one);
    assert_out_of_range(&lab, "SELECT -x FROM t ORDER BY 0", one);
    assert_out_of_range(&lab, "SELECT -x FROM t ORDER BY -1", one);
    assert_out_of_range(&lab, "SELECT -x FROM t ORDER BY -0", one);
    assert_out_of_range(&lab, "SELECT -x FROM t ORDER BY 2147483647", one);
    assert_out_of_range(
        &lab,
        "SELECT x, g FROM t ORDER BY x, 3",
        "2nd ORDER BY term out of range - should be between 1 and 2",
    );
    assert_out_of_range(&lab, "SELECT -x FROM t UNION SELECT 1 ORDER BY 2", one);
    // Past i32, SQLite reads the literal as a constant, not a position.
    lab.assert_same("SELECT -x FROM t ORDER BY 2147483648, 1", true);
}

/// A position whose result column is a plain column keeps the index
/// orderings (the term is bound to the column itself).
#[test]
fn new_01_order_by_ordinal_through_an_index() {
    let lab = lab();
    lab.exec_both("CREATE INDEX t_x ON t(x); CREATE INDEX t_y ON t(y DESC);");
    for sql in [
        "SELECT x FROM t ORDER BY 1 LIMIT 2",
        "SELECT x, g FROM t ORDER BY 1 DESC LIMIT 3",
        "SELECT y, -x FROM t ORDER BY 1 LIMIT 2",
        "SELECT -x, x FROM t ORDER BY 2 LIMIT 2",
        "SELECT x FROM t WHERE x > 1 ORDER BY 1",
    ] {
        lab.assert_same(sql, true);
    }
}

/// A bound parameter or a constant expression is not a position; SQLite
/// sorts by the constant, which leaves the rows in scan order.
#[test]
fn new_01_order_by_constants_are_not_positions() {
    let lab = lab();
    lab.assert_same_bound("SELECT -x FROM t ORDER BY ?", &[int(2)], false);
    lab.assert_same("SELECT -x FROM t ORDER BY 1 + 0", false);
    lab.assert_same("SELECT g, count(*) FROM t GROUP BY g ORDER BY 1 + 1", false);
    lab.assert_same("SELECT DISTINCT -x FROM t ORDER BY 0 + 1", false);
}

/// Enough rows for the one-pass hash aggregate (16 or more).
#[test]
fn new_01_order_by_ordinal_one_pass_grouping() {
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE big(g TEXT, v INTEGER); \
         WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < 59) \
         INSERT INTO big SELECT char(65 + (i % 3) + 32 * ((i / 3) % 2)), i FROM n;",
    );
    for sql in [
        "SELECT g, count(*), sum(v) FROM big GROUP BY g ORDER BY 3 DESC",
        "SELECT g, sum(v) FROM big GROUP BY g ORDER BY 2, 1",
        "SELECT g, -sum(v) FROM big GROUP BY g ORDER BY 2",
        "SELECT g, sum(v) FROM big GROUP BY g ORDER BY 1 COLLATE NOCASE DESC, 2",
        "SELECT upper(g), sum(v) FROM big GROUP BY g ORDER BY 1, 2 DESC",
    ] {
        lab.assert_same(sql, true);
    }
}
