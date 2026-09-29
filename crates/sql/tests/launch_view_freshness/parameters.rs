//! Parameters bound after preparation reach a derived table and a subquery,
//! numbered in statement order.

use super::*;

#[test]
fn derived_table_parameter() {
    let lab = setup();
    lab.exec_both("INSERT INTO t VALUES (7), (9);");
    // A parameter inside the derived table, and parameters on both sides
    // of it, keep the numbering of the whole statement.
    for (sql, params) in [
        (
            "SELECT a FROM (SELECT a FROM t WHERE a = ?) ORDER BY a",
            vec![7i64],
        ),
        (
            "SELECT ?, s.a FROM (SELECT a FROM t WHERE a = ?) s WHERE s.a > ?",
            vec![100, 9, 1],
        ),
        (
            "SELECT s.a FROM t JOIN (SELECT a FROM t WHERE a >= ?2) s ON s.a = t.a \
             WHERE t.a <= ?1 ORDER BY s.a",
            vec![9, 7],
        ),
    ] {
        let mut held = lab.redline.prepare(sql).expect("prepare");
        for (i, value) in params.iter().enumerate() {
            held.bind_i64(i + 1, *value).expect("bind");
        }
        let mut oracle = lab.sqlite.prepare(sql).expect("sqlite prepare");
        assert_eq!(
            rows_of(&mut held),
            sqlite_rows(&mut oracle, &params),
            "{sql}"
        );
    }
    let named = "SELECT x FROM (SELECT :v AS x) WHERE x = :v";
    let mut held = lab.redline.prepare(named).expect("prepare");
    held.bind_named(":v", SqlValue::Integer(3)).expect("bind");
    assert_eq!(rows_of(&mut held), vec![vec![int(3)]]);
}

/// A subquery is bound in a pass of its own; its parameters used to be
/// numbered from 1 again, so `SELECT ?, (SELECT ?)` had one parameter and
/// read the first value twice.
#[test]
fn subquery_parameters_keep_the_statement_numbering() {
    let lab = setup();
    lab.exec_both("INSERT INTO t VALUES (2), (3);");
    for (sql, params) in [
        ("SELECT ?, (SELECT ?)", vec![1i64, 11]),
        (
            "SELECT a FROM t WHERE a IN (SELECT ? UNION SELECT ?) AND a <> ? ORDER BY a",
            vec![1, 3, 3],
        ),
        ("SELECT (SELECT ?2), ?1", vec![5, 6]),
        (
            "SELECT count(*) FROM t WHERE EXISTS (SELECT 1 WHERE ? = 1) AND a > ?",
            vec![1, 1],
        ),
    ] {
        let mut held = lab.redline.prepare(sql).expect("prepare");
        assert_eq!(held.parameter_count(), params.len(), "{sql}");
        for (i, value) in params.iter().enumerate() {
            held.bind_i64(i + 1, *value).expect("bind");
        }
        let mut oracle = lab.sqlite.prepare(sql).expect("sqlite prepare");
        assert_eq!(
            rows_of(&mut held),
            sqlite_rows(&mut oracle, &params),
            "{sql}"
        );
    }
    let named = "SELECT (SELECT :a), :b, :a";
    let mut held = lab.redline.prepare(named).expect("prepare");
    assert_eq!(held.parameter_count(), 2);
    held.bind_named(":a", SqlValue::Integer(1)).expect("bind a");
    held.bind_named(":b", SqlValue::Integer(11))
        .expect("bind b");
    assert_eq!(rows_of(&mut held), vec![vec![int(1), int(11), int(1)]]);
}
