//! NEW-02: GROUP BY, PARTITION BY and DISTINCT aggregates put INTEGER 1 and
//! REAL 1.0 in one group.
//!
//! Group keys were stored-record bytes, which keep the storage class, so
//! `SELECT x FROM (SELECT 1 x UNION ALL SELECT 1.0) GROUP BY x` answered
//! two groups where SQLite answers one. SQLite shows the group's first
//! row, so the group of `1.0, 1` shows REAL 1.0.

use crate::lab::{Lab, int, real, text};

#[test]
fn new_02_group_by_int_and_real_is_one_group() {
    let lab = Lab::new();
    lab.assert_rows(
        "SELECT x FROM (SELECT 1 x UNION ALL SELECT 1.0) GROUP BY x",
        &[vec![int(1)]],
    );
    lab.assert_rows(
        "SELECT x, typeof(x), count(*) FROM (SELECT 1.0 x UNION ALL SELECT 1) GROUP BY x",
        &[vec![real(1.0), text("real"), int(2)]],
    );
    lab.assert_rows(
        "SELECT count(*) FROM (SELECT x FROM (SELECT 0 x UNION ALL SELECT 0.0 \
         UNION ALL SELECT -0.0 UNION ALL SELECT 1.5 UNION ALL SELECT 1) GROUP BY x)",
        &[vec![int(3)]],
    );
}

fn stored_lab() -> Lab {
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE t(x, y TEXT, z INTEGER); \
         INSERT INTO t VALUES (1, 'a', 10), (1.0, 'b', 20), (2.0, 'c', 30), (2, 'd', 40), \
         ('1', 'e', 50), (1.5, 'f', 60), (NULL, 'g', 70), (9007199254740993, 'h', 80), \
         (9007199254740992.0, 'i', 90);",
    );
    lab
}

#[test]
fn new_02_stored_values_group_by_value() {
    let lab = stored_lab();
    lab.assert_same(
        "SELECT x, typeof(x), count(*), sum(z) FROM t GROUP BY x",
        false,
    );
    lab.assert_same(
        "SELECT x, typeof(x), count(*) FROM t GROUP BY x ORDER BY count(*) DESC, x",
        true,
    );
    lab.assert_same(
        "SELECT x, count(*) FROM t GROUP BY x HAVING count(*) > 1",
        false,
    );
    lab.assert_same("SELECT count(*) FROM (SELECT x FROM t GROUP BY x)", true);
    lab.assert_same("SELECT x + 0, count(*) FROM t GROUP BY x + 0", false);
    lab.assert_same(
        "SELECT x, z > 25, count(*) FROM t GROUP BY x, z > 25",
        false,
    );
}

/// The group shows its first row's values, key and bare columns alike.
#[test]
fn new_02_group_shows_first_row() {
    let lab = stored_lab();
    lab.assert_same("SELECT x, typeof(x), y FROM t GROUP BY x", false);
}

#[test]
fn new_02_distinct_aggregates_and_partitions() {
    let lab = stored_lab();
    lab.assert_same(
        "SELECT count(DISTINCT x), count(DISTINCT x + 0) FROM t",
        true,
    );
    lab.assert_same("SELECT y, count(*) OVER (PARTITION BY x) FROM t", false);
    lab.assert_same("SELECT DISTINCT x + 0 FROM t", false);
    lab.assert_rows(
        "SELECT sum(DISTINCT x), count(DISTINCT x) \
         FROM (SELECT 1 x UNION ALL SELECT 1.0 UNION ALL SELECT 2)",
        &[vec![int(3), int(2)]],
    );
    lab.assert_rows(
        "SELECT sum(DISTINCT x), count(DISTINCT x) \
         FROM (SELECT 1.0 x UNION ALL SELECT 1 UNION ALL SELECT 2)",
        &[vec![real(3.0), int(2)]],
    );
}

/// Enough rows for the hash aggregate and the vectorized paths, with each
/// group seen as INTEGER and as REAL.
#[test]
fn new_02_many_rows_group_by_value() {
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE big(x, v INTEGER); \
         WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < 3999) \
         INSERT INTO big SELECT CASE WHEN (i / 100) % 2 = 0 THEN i % 100 ELSE (i % 100) * 1.0 END, i \
         FROM n;",
    );
    lab.assert_rows(
        "SELECT count(*) FROM (SELECT x FROM big GROUP BY x)",
        &[vec![int(100)]],
    );
    lab.assert_same(
        "SELECT x, typeof(x), count(*), sum(v) FROM big GROUP BY x",
        false,
    );
    lab.assert_same("SELECT x, count(*) FROM big GROUP BY x ORDER BY x", true);
    lab.assert_same("SELECT count(DISTINCT x), count(*) FROM big", true);
}

/// The same groups when the rows come through an index on the key.
#[test]
fn new_02_indexed_group_by_value() {
    let lab = stored_lab();
    lab.exec_both("CREATE INDEX t_x ON t(x);");
    lab.assert_same(
        "SELECT x, count(*), sum(z) FROM t INDEXED BY t_x WHERE x > 0 GROUP BY x",
        false,
    );
    lab.assert_same(
        "SELECT x, count(*) FROM t INDEXED BY t_x GROUP BY x ORDER BY x",
        true,
    );
    lab.assert_same(
        "SELECT x, count(*), sum(z) FROM t NOT INDEXED WHERE x > 0 GROUP BY x",
        false,
    );
}
