//! Differential matrices for GROUP BY with HAVING, window functions and
//! subqueries, run on the parent file's `Lab`.

use super::*;

#[test]
fn diff_group_by_having_matrix() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE t(grp TEXT, v INTEGER)");
    lab.execute(
        "INSERT INTO t VALUES ('A', 1), ('A', 2), ('A', NULL), ('B', NULL), ('B', 4), ('C', 5)",
    );

    lab.assert_queries(&[
        "SELECT grp, count(*), count(v) FROM t GROUP BY grp HAVING count(*) >= 2 ORDER BY grp",
        "SELECT grp, sum(v), total(v) FROM t GROUP BY grp HAVING sum(COALESCE(v, 0)) >= 3 ORDER BY grp",
        "SELECT grp, min(v), max(v) FROM t GROUP BY grp HAVING max(v) IS NOT NULL ORDER BY grp",
        "SELECT grp, avg(v) FROM t GROUP BY grp HAVING avg(v) > 1 ORDER BY grp",
        "SELECT grp, count(*) FROM t GROUP BY grp HAVING count(v) < count(*) ORDER BY grp",
        "SELECT grp, sum(COALESCE(v, 0)) FROM t GROUP BY grp HAVING count(v) BETWEEN 1 AND 2 ORDER BY grp",
        "SELECT grp, total(v) FROM t GROUP BY grp HAVING total(v) >= 4.0 ORDER BY grp",
        "SELECT grp, count(*) FROM t GROUP BY grp HAVING grp IN ('A', 'B', 'C') ORDER BY grp",
        "SELECT grp, min(COALESCE(v, -1)) FROM t GROUP BY grp HAVING min(COALESCE(v, -1)) <= 1 ORDER BY grp",
        "SELECT grp, max(v) FROM t GROUP BY grp HAVING sum(COALESCE(v, 0)) <> 0 ORDER BY grp",
    ]);
}

#[test]
fn diff_window_matrix() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, grp TEXT, v INTEGER)");
    lab.execute(
        "INSERT INTO t(id, grp, v) VALUES (1, 'A', 10), (2, 'A', NULL), (3, 'A', 30), \
         (4, 'B', NULL), (5, 'B', 50), (6, 'B', 60)",
    );

    lab.assert_queries(&[
        "SELECT id, grp, v, row_number() OVER (PARTITION BY grp ORDER BY id) FROM t ORDER BY id",
        "SELECT id, grp, v, lag(v) OVER (PARTITION BY grp ORDER BY id) FROM t ORDER BY id",
        "SELECT id, grp, v, lead(v) OVER (PARTITION BY grp ORDER BY id) FROM t ORDER BY id",
        "SELECT id, grp, v, sum(COALESCE(v, 0)) OVER (PARTITION BY grp ORDER BY id ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) FROM t ORDER BY id",
        "SELECT id, grp, v, rank() OVER (PARTITION BY grp ORDER BY v) FROM t ORDER BY grp, v, id",
        "SELECT id, grp, v, dense_rank() OVER (PARTITION BY grp ORDER BY v) FROM t ORDER BY grp, v, id",
        "SELECT id, grp, v, first_value(v) OVER (PARTITION BY grp ORDER BY id ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) FROM t ORDER BY id",
        "SELECT id, grp, v, last_value(v) OVER (PARTITION BY grp ORDER BY id ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) FROM t ORDER BY id",
        "SELECT id, grp, v, nth_value(v, 2) OVER (PARTITION BY grp ORDER BY id ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) FROM t ORDER BY id",
        "SELECT id, grp, v, count(*) OVER (PARTITION BY grp) FROM t ORDER BY id",
        "SELECT id, grp, v, avg(v) OVER (PARTITION BY grp ORDER BY id ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING) FROM t ORDER BY id",
    ]);
}

#[test]
fn diff_subquery_matrix() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE a(id INTEGER PRIMARY KEY, v TEXT)");
    lab.execute("CREATE TABLE b(aid INTEGER, v TEXT)");
    lab.execute("INSERT INTO a VALUES (1, 'A1'), (2, NULL), (3, 'A3')");
    lab.execute("INSERT INTO b VALUES (1, 'B1'), (1, NULL), (3, 'B3'), (4, 'B4')");

    lab.assert_queries(&[
        "SELECT id FROM a WHERE id IN (SELECT aid FROM b WHERE v IS NOT NULL) ORDER BY id",
        "SELECT id FROM a WHERE id NOT IN (SELECT aid FROM b WHERE aid IS NOT NULL AND v IS NOT NULL) ORDER BY id",
        "SELECT a.id FROM a WHERE EXISTS (SELECT 1 FROM b WHERE b.aid = a.id AND b.v IS NULL) ORDER BY a.id",
        "SELECT a.id FROM a WHERE NOT EXISTS (SELECT 1 FROM b WHERE b.aid = a.id AND b.v IS NULL) ORDER BY a.id",
        "SELECT a.id, (SELECT COUNT(*) FROM b WHERE b.aid = a.id) AS bcnt FROM a ORDER BY a.id",
        "SELECT id, (SELECT v FROM b WHERE b.aid = a.id ORDER BY v LIMIT 1) FROM a ORDER BY id",
    ]);
}
