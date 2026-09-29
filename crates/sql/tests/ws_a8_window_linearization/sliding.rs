//! Bounded sliding-window path (n PRECEDING AND m FOLLOWING), against rusqlite.

use super::*;

#[test]
fn sliding_sum_one_each_side() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE t(id INTEGER, v INTEGER)");
    lab.execute("INSERT INTO t VALUES (1,1),(2,2),(3,3),(4,4),(5,5)");
    lab.assert_match(
        "SELECT id, v, SUM(v) OVER (ORDER BY id ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING) \
         FROM t ORDER BY id",
    );
}

#[test]
fn sliding_avg_two_each_side() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE t(id INTEGER, v REAL)");
    lab.execute("INSERT INTO t VALUES (1,1.0),(2,2.0),(3,3.0),(4,4.0),(5,5.0),(6,6.0)");
    lab.assert_match(
        "SELECT id, v, AVG(v) OVER (ORDER BY id ROWS BETWEEN 2 PRECEDING AND 2 FOLLOWING) \
         FROM t ORDER BY id",
    );
}

#[test]
fn sliding_count_partitioned() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE t(p INTEGER, id INTEGER, v INTEGER)");
    lab.execute(
        "INSERT INTO t VALUES \
         (1,1,10),(1,2,20),(1,3,30),(1,4,40), \
         (2,1,100),(2,2,200),(2,3,300)",
    );
    lab.assert_match(
        "SELECT p, id, v, \
            COUNT(*) OVER (PARTITION BY p ORDER BY id \
                           ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING) \
         FROM t ORDER BY p, id",
    );
}

#[test]
fn sliding_sum_asymmetric_bounds() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE t(id INTEGER, v INTEGER)");
    lab.execute("INSERT INTO t VALUES (1,1),(2,2),(3,3),(4,4),(5,5),(6,6),(7,7)");
    lab.assert_match(
        "SELECT id, v, SUM(v) OVER (ORDER BY id ROWS BETWEEN 2 PRECEDING AND 1 FOLLOWING) \
         FROM t ORDER BY id",
    );
}

#[test]
fn sliding_sum_with_nulls() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE t(id INTEGER, v INTEGER)");
    lab.execute("INSERT INTO t VALUES (1,1),(2,NULL),(3,3),(4,NULL),(5,5)");
    lab.assert_match(
        "SELECT id, v, SUM(v) OVER (ORDER BY id ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING), \
                       COUNT(v) OVER (ORDER BY id ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING) \
         FROM t ORDER BY id",
    );
}
