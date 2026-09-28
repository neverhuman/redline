//! SQLite comparison affinity (datatype3.html §4.2) in WHERE, IN, BETWEEN,
//! IS [NOT] DISTINCT FROM, CASE, joins, subqueries, index probes and the
//! rowid fast path.
//!
//! RedlineDB compared operands with no conversion at all: in an INTEGER
//! column `WHERE x = '5'` found nothing (SQLite finds 5), in a TEXT column
//! `WHERE y = 5` found nothing, `x IN ('5')` and `x BETWEEN '4' AND '6'`
//! missed rows, joins on `a.x = b.y` matched nothing, and `WHERE rowid = '1'`
//! was refused with `UnsupportedSql`. Every query here must match bundled
//! SQLite row for row and storage class for storage class, on a scan and
//! through an index.

use crate::lab::{Lab, int, null, real, text};

/// Columns of every affinity, holding numbers, numeric text and text.
fn lab() -> Lab {
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE t(id INTEGER PRIMARY KEY, x INTEGER, r REAL, n NUMERIC, y TEXT, z, b BLOB);\
         INSERT INTO t VALUES(1, 5, 5.0, 5, '5', 5, 5);\
         INSERT INTO t VALUES(2, 10, 10.5, '10', '10', '10', '10');\
         INSERT INTO t VALUES(3, 7, 7.0, 'abc', 'abc', 'abc', x'35');\
         INSERT INTO t VALUES(4, 20, 2.5, 2.5, '2.5', 2.5, NULL);\
         INSERT INTO t VALUES(5, NULL, NULL, NULL, ' 5', ' 5', 'x');",
    );
    lab
}

const COLUMNS: [&str; 7] = ["id", "x", "r", "n", "y", "z", "b"];
const PROBES: [&str; 12] = [
    "'5'", "5", "'5.0'", "5.0", "' 5 '", "'+5'", "'5e0'", "'5x'", "'abc'", "'10'", "10", "'2.5'",
];
const OPERATORS: [&str; 6] = ["=", "<>", "<", "<=", ">", ">="];

#[test]
fn comparison_affinity_every_column_operator_and_probe() {
    let lab = lab();
    for column in COLUMNS {
        for probe in PROBES {
            for op in OPERATORS {
                lab.assert_same(
                    &format!("SELECT id FROM t WHERE {column} {op} {probe} ORDER BY id"),
                    true,
                );
                lab.assert_same(
                    &format!("SELECT id FROM t WHERE {probe} {op} {column} ORDER BY id"),
                    true,
                );
            }
            lab.assert_same(
                &format!(
                    "SELECT id, {column} = {probe}, {column} IS NOT DISTINCT FROM {probe} \
                     FROM t ORDER BY id"
                ),
                true,
            );
        }
    }
    // The witnesses from the audit.
    lab.assert_rows("SELECT id FROM t WHERE x = '5'", &[vec![int(1)]]);
    lab.assert_rows("SELECT id FROM t WHERE y = 5", &[vec![int(1)]]);
    lab.assert_rows("SELECT count(*) FROM t WHERE z = '5'", &[vec![int(0)]]);
}

#[test]
fn comparison_affinity_in_lists_use_the_left_operand() {
    let lab = lab();
    for column in COLUMNS {
        for list in [
            "('5', '10')",
            "(5, 10)",
            "('5.0', 7)",
            "('abc', 2.5)",
            "(' 5')",
        ] {
            lab.assert_same(
                &format!("SELECT id FROM t WHERE {column} IN {list} ORDER BY id"),
                true,
            );
            lab.assert_same(
                &format!("SELECT id FROM t WHERE {column} NOT IN {list} ORDER BY id"),
                true,
            );
        }
    }
    // A literal on the left has no affinity, whatever the list holds.
    lab.assert_same("SELECT id FROM t WHERE '5' IN (x, y, z) ORDER BY id", true);
    lab.assert_same("SELECT id FROM t WHERE 5 IN (y, z) ORDER BY id", true);
}

#[test]
fn comparison_affinity_between_each_bound() {
    let lab = lab();
    for column in COLUMNS {
        for (low, high) in [("'4'", "'6'"), ("4", "6"), ("'2'", "11"), ("1", "'abc'")] {
            lab.assert_same(
                &format!("SELECT id FROM t WHERE {column} BETWEEN {low} AND {high} ORDER BY id"),
                true,
            );
            lab.assert_same(
                &format!(
                    "SELECT id FROM t WHERE {column} NOT BETWEEN {low} AND {high} ORDER BY id"
                ),
                true,
            );
        }
    }
}

#[test]
fn comparison_affinity_is_distinct_case_and_cast() {
    let lab = lab();
    lab.assert_same(
        "SELECT id, x IS NOT DISTINCT FROM '5', y IS DISTINCT FROM 5, z IS NOT DISTINCT FROM '5' \
         FROM t ORDER BY id",
        true,
    );
    lab.assert_same(
        "SELECT id, CASE x WHEN '5' THEN 'five' WHEN '10' THEN 'ten' ELSE 'other' END, \
         CASE y WHEN 5 THEN 'five' ELSE 'other' END, CASE z WHEN '5' THEN 'five' END \
         FROM t ORDER BY id",
        true,
    );
    lab.assert_rows(
        "SELECT CAST(5 AS INTEGER) = '5', 5 = '5', CAST('5' AS TEXT) = 5, '5' = 5",
        &[vec![int(1), int(0), int(1), int(0)]],
    );
    lab.assert_same(
        "SELECT id, y = CAST(5 AS INTEGER), z = CAST('5' AS INTEGER), x = CAST(y AS TEXT), \
         CAST(x AS TEXT) = y FROM t ORDER BY id",
        true,
    );
    // `+x` strips the column's affinity.
    lab.assert_same("SELECT id FROM t WHERE +x = '5' ORDER BY id", true);
}

#[test]
fn comparison_affinity_between_two_columns_and_in_joins() {
    let lab = lab();
    lab.exec_both(
        "CREATE TABLE u(k INTEGER, s TEXT, w);\
         INSERT INTO u VALUES(5, '5', '5'), (10, '10.0', 10), (7, 'abc', 'abc');",
    );
    for (left, right) in [
        ("t.x", "u.s"),
        ("t.y", "u.k"),
        ("t.z", "u.k"),
        ("t.z", "u.s"),
        ("t.y", "u.w"),
        ("t.x", "u.w"),
        ("t.n", "u.s"),
    ] {
        lab.assert_same(
            &format!("SELECT t.id, u.k FROM t JOIN u ON {left} = {right} ORDER BY 1, 2"),
            true,
        );
        lab.assert_same(
            &format!("SELECT t.id, u.k FROM t, u WHERE {right} = {left} ORDER BY 1, 2"),
            true,
        );
    }
    // The join index probe must not bypass the comparison's affinity.
    lab.exec_both("CREATE INDEX u_s ON u(s); CREATE INDEX u_k ON u(k); CREATE INDEX u_w ON u(w)");
    for (left, right) in [
        ("t.x", "u.s"),
        ("t.y", "u.k"),
        ("t.z", "u.s"),
        ("t.y", "u.w"),
    ] {
        lab.assert_same(
            &format!("SELECT t.id, u.k FROM t JOIN u ON {left} = {right} ORDER BY 1, 2"),
            true,
        );
    }
}

#[test]
fn comparison_affinity_in_and_scalar_subqueries() {
    let lab = lab();
    for (column, sub) in [
        ("x", "y"),
        ("y", "x"),
        ("z", "x"),
        ("x", "z"),
        ("y", "z"),
        ("id", "y"),
    ] {
        lab.assert_same(
            &format!("SELECT id FROM t WHERE {column} IN (SELECT {sub} FROM t) ORDER BY id"),
            true,
        );
        lab.assert_same(
            &format!("SELECT id FROM t WHERE {column} = (SELECT {sub} FROM t WHERE id = 1)"),
            true,
        );
    }
    lab.assert_same(
        "SELECT id FROM t WHERE '5' IN (SELECT x FROM t) ORDER BY id",
        true,
    );
    lab.assert_same(
        "SELECT '5' IN (SELECT x FROM t), 5 IN (SELECT y FROM t)",
        true,
    );
    // A correlated reference keeps the outer column's affinity.
    lab.assert_same(
        "SELECT id, (SELECT count(*) FROM t AS i WHERE i.x = o.y) FROM t AS o ORDER BY id",
        true,
    );
}

#[test]
fn comparison_affinity_indexed_lookups_match_scans() {
    let lab = lab();
    for column in ["x", "r", "n", "y", "z"] {
        lab.exec_both(&format!("CREATE INDEX t_{column} ON t({column})"));
        for probe in PROBES {
            for op in OPERATORS {
                lab.indexed_vs_scan(
                    "t",
                    &format!("t_{column}"),
                    &format!("SELECT id FROM {{access}} WHERE {column} {op} {probe}"),
                );
            }
            lab.indexed_vs_scan(
                "t",
                &format!("t_{column}"),
                &format!("SELECT id FROM {{access}} WHERE {column} BETWEEN {probe} AND '99'"),
            );
        }
    }
    // An ordered, limited walk of the index converts its probe too.
    lab.assert_same("SELECT id FROM t WHERE x > '6' ORDER BY x LIMIT 2", true);
    lab.assert_same("SELECT id FROM t WHERE y > 3 ORDER BY y LIMIT 2", true);
}

#[test]
fn comparison_affinity_rowid_lookups() {
    let lab = lab();
    for probe in [
        "'1'", "' 1 '", "'1.0'", "'+1'", "'1e0'", "'1x'", "'abc'", "1.0", "'1.5'", "'-1'",
    ] {
        for column in ["id", "rowid", "oid", "_rowid_"] {
            lab.assert_same(&format!("SELECT id FROM t WHERE {column} = {probe}"), true);
        }
        lab.assert_same(&format!("SELECT id FROM t WHERE {probe} = id"), true);
    }
    for bound in [
        text("2"),
        text("2.0"),
        text(" 2"),
        text("2x"),
        real(2.0),
        int(2),
    ] {
        lab.assert_same_bound("SELECT id, y FROM t WHERE id = ?1", &[bound.clone()], true);
        lab.assert_same_bound("SELECT id FROM t WHERE rowid = ?1", &[bound], true);
    }
}

#[test]
fn comparison_affinity_bound_parameters() {
    let lab = lab();
    for bound in [
        text("5"),
        text("5.0"),
        text(" 5"),
        int(5),
        real(5.0),
        text("abc"),
        null(),
    ] {
        for column in COLUMNS {
            lab.assert_same_bound(
                &format!("SELECT id FROM t WHERE {column} = ?1 ORDER BY id"),
                &[bound.clone()],
                true,
            );
            lab.assert_same_bound(
                &format!("SELECT id FROM t WHERE {column} IN (?1, 99) ORDER BY id"),
                &[bound.clone()],
                true,
            );
        }
    }
}

#[test]
fn comparison_affinity_in_update_delete_and_having() {
    let lab = lab();
    lab.step_both("UPDATE t SET y = 'five' WHERE x = '5'");
    lab.step_both("DELETE FROM t WHERE y = 10");
    lab.step_both("UPDATE t SET z = 'hit' WHERE n IN ('2.5', 'abc')");
    lab.assert_same("SELECT id, x, y, z FROM t ORDER BY id", true);
    lab.assert_same(
        "SELECT x, count(*) FROM t GROUP BY x HAVING x = '5' OR x > '7' ORDER BY x",
        true,
    );
}

#[test]
fn comparison_affinity_trigger_new_and_old_have_none() {
    // SQLite resolves NEW.col and OLD.col without affinity (TK_TRIGGER);
    // only the rowid is INTEGER. A plain table column in the body keeps
    // its own affinity.
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE t(x INTEGER, y TEXT);\
         CREATE TABLE log(m TEXT);\
         CREATE TRIGGER a AFTER INSERT ON t WHEN new.x = '9' BEGIN INSERT INTO log VALUES('x'); END;\
         CREATE TRIGGER b AFTER INSERT ON t WHEN new.y = 9 BEGIN INSERT INTO log VALUES('y'); END;\
         CREATE TRIGGER c AFTER INSERT ON t WHEN new.rowid = '1' BEGIN INSERT INTO log VALUES('rowid'); END;\
         CREATE TRIGGER d AFTER INSERT ON t BEGIN INSERT INTO log SELECT 'col' FROM t WHERE t.x = new.y; END;\
         INSERT INTO t VALUES(9, '9');",
    );
    lab.assert_rows(
        "SELECT m FROM log ORDER BY m",
        &[vec![text("col")], vec![text("rowid")]],
    );
}

#[test]
fn comparison_affinity_subquery_literal_columns_have_none() {
    let lab = lab();
    lab.assert_rows(
        "SELECT c = 5, d = '5' FROM (SELECT '5' AS c, 5 AS d)",
        &[vec![int(0), int(0)]],
    );
}

#[test]
fn comparison_affinity_partial_index_predicate() {
    let lab = lab();
    lab.exec_both("CREATE INDEX t_part ON t(id) WHERE x = '5' OR y = 10");
    lab.assert_same(
        "SELECT id FROM t INDEXED BY t_part WHERE x = '5' OR y = 10 ORDER BY id",
        true,
    );
    lab.assert_same(
        "SELECT id FROM t NOT INDEXED WHERE x = '5' OR y = 10 ORDER BY id",
        true,
    );
}

/// An equijoin read through the right table's index converts exactly as
/// the comparison does: two TEXT columns convert nothing (and may use the
/// index), a column without a type against a TEXT column converts nothing
/// either, so INTEGER 5 does not meet TEXT '5' however the join runs.
#[test]
fn equijoin_through_an_index_keeps_the_comparison_affinity() {
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE tt(id INTEGER PRIMARY KEY, y TEXT); CREATE INDEX tt_y ON tt(y); \
         INSERT INTO tt VALUES (1, '5'), (2, 'abc'), (3, '7.0'), (4, '7'); \
         CREATE TABLE tb(id INTEGER PRIMARY KEY, w); \
         INSERT INTO tb VALUES (1, 5), (2, '5'), (3, 'abc'), (4, 7), (5, x'35'), (6, 7.0); \
         CREATE TABLE tn(id INTEGER PRIMARY KEY, n INTEGER); CREATE INDEX tn_n ON tn(n); \
         INSERT INTO tn VALUES (1, 5), (2, 7); \
         CREATE TABLE t2(id INTEGER PRIMARY KEY, z TEXT); \
         INSERT INTO t2 VALUES (1, '5'), (2, 'abc'), (3, '7.0'), (4, 'x'), (5, '7');",
    );
    for sql in [
        "SELECT tb.id, tt.id FROM tb JOIN tt ON tb.w = tt.y",
        "SELECT t2.id, tt.id FROM t2 JOIN tt ON t2.z = tt.y",
        "SELECT t2.id, tt.id FROM t2 JOIN tt ON tt.y = t2.z",
        "SELECT t2.id, tn.id FROM t2 JOIN tn ON t2.z = tn.n",
        "SELECT tb.id, tn.id FROM tb JOIN tn ON tb.w = tn.n",
        "SELECT tt.id, tb.id FROM tt JOIN tb ON tt.y = tb.w",
        "SELECT tb.id, tt.id FROM tb JOIN tt ON tb.w = tt.y WHERE tt.y > ''",
    ] {
        lab.assert_same(sql, false);
    }
}

/// A comparison in HAVING or in a grouped SELECT list with an aggregate on
/// one side: the aggregate has no affinity and a GROUP BY column keeps its
/// own, so `g = count(*)` over a TEXT column compares as TEXT (`'2' =
/// 2`). The grouped evaluator compared the raw values, so these found
/// nothing.
#[test]
fn comparison_affinity_against_aggregates() {
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE ga(g TEXT, v INTEGER, w); \
         INSERT INTO ga VALUES ('1', 1, 5), ('2', 2, 5), ('2', 3, '5'), ('10', 4, 6), ('10', 6, 6); \
         CREATE TABLE gb(a INTEGER, b TEXT); \
         INSERT INTO gb VALUES (1, '1'), (2, '3'), (3, '3'), (3, '2');",
    );
    for sql in [
        "SELECT g FROM ga GROUP BY g HAVING g = count(*)",
        "SELECT g, g = count(*), count(*) = g FROM ga GROUP BY g",
        "SELECT g FROM ga GROUP BY g HAVING sum(v) = g",
        "SELECT g, sum(v) = g, sum(v) < g, sum(v) >= g, sum(v) <> g FROM ga GROUP BY g",
        "SELECT g FROM ga GROUP BY g HAVING sum(v) BETWEEN g AND g",
        "SELECT g, count(*) BETWEEN g AND 5 FROM ga GROUP BY g",
        "SELECT g FROM ga GROUP BY g HAVING g IN (count(*), sum(v))",
        "SELECT g, g NOT IN (count(*), 99) FROM ga GROUP BY g",
        "SELECT a FROM gb GROUP BY a HAVING a = max(b)",
        "SELECT a, a = max(b), a < min(b) FROM gb GROUP BY a",
        "SELECT w, count(*) = w FROM ga GROUP BY w",
        "SELECT g, CAST(sum(v) AS TEXT) = g FROM ga GROUP BY g",
        "SELECT g, count(*) IN ('1', '2') FROM ga GROUP BY g",
        "SELECT g, NULL IN (count(*)), count(*) NOT IN (NULL, count(*)) FROM ga GROUP BY g",
    ] {
        lab.assert_same(sql, false);
    }
    lab.assert_rows(
        "SELECT g FROM ga GROUP BY g HAVING sum(v) = g ORDER BY g",
        &[vec![text("1")], vec![text("10")]],
    );
}
