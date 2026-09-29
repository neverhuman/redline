//! total(): its sum, and the REAL zero it answers for NULL-only and empty input.

use super::*;

#[test]
fn total_basic_sum() {
    let (_d, c) = open();
    c.execute("CREATE TABLE t(n REAL)").expect("create");
    c.execute("INSERT INTO t VALUES (1.0), (2.0), (3.0)")
        .expect("insert");
    let v = q1(&c, "SELECT total(n) FROM t");
    assert_eq!(v, SqlValue::Real(6.0));
}

#[test]
fn total_all_null_returns_zero_real() {
    // SQLite: total(X) returns 0.0 for all-NULL groups, unlike sum() which returns NULL.
    let (_d, c) = open();
    c.execute("CREATE TABLE t(n INTEGER)").expect("create");
    c.execute("INSERT INTO t VALUES (NULL), (NULL)")
        .expect("insert");
    let v = q1(&c, "SELECT total(n) FROM t");
    assert_eq!(v, SqlValue::Real(0.0));
}

#[test]
fn total_empty_table_returns_zero_real() {
    let (_d, c) = open();
    c.execute("CREATE TABLE t(n INTEGER)").expect("create");
    let v = q1(&c, "SELECT total(n) FROM t");
    assert_eq!(v, SqlValue::Real(0.0));
}

#[test]
fn total_skips_null_values() {
    let (_d, c) = open();
    c.execute("CREATE TABLE t(n INTEGER)").expect("create");
    c.execute("INSERT INTO t VALUES (10), (NULL), (5)")
        .expect("insert");
    let v = q1(&c, "SELECT total(n) FROM t");
    assert_eq!(v, SqlValue::Real(15.0));
}
