//! Q5-08: rows a statement reads through a view, a CTE or a derived table
//! are the rows of the moment it runs, with the parameters bound for that
//! run.
//!
//! The binder materializes those row sources when a statement is prepared.
//! A statement prepared before an insert, or reset and stepped again,
//! returned the rows of its preparation, and a parameter inside them was
//! always NULL, because the materialization ran before anything was bound:
//! `WITH c AS (SELECT :x AS x) SELECT x FROM c` answered NULL with :x = 7.
//! A statement whose binding materialized rows now binds again when it is
//! executed, with its current parameters, unless it runs straight after its
//! preparation with nothing bound and nothing written in between. Every
//! expectation is checked against bundled SQLite.

#[path = "launch_wrong_answer/lab.rs"]
#[allow(dead_code)]
mod lab;

use lab::{Lab, int};
use redlinedb_sql::{SqlValue, Statement, Step};

fn rows_of(stmt: &mut Statement) -> Vec<Vec<SqlValue>> {
    let mut out = Vec::new();
    while let Step::Row = stmt.step().expect("step") {
        out.push(
            (0..stmt.column_count())
                .map(|i| stmt.column_value(i).expect("column").clone())
                .collect(),
        );
    }
    out
}

/// Rows of a held rusqlite statement, for the same prepare-then-change
/// sequence on the oracle.
fn sqlite_rows(stmt: &mut rusqlite::Statement<'_>, params: &[i64]) -> Vec<Vec<SqlValue>> {
    for (i, value) in params.iter().enumerate() {
        stmt.raw_bind_parameter(i + 1, *value).expect("bind");
    }
    let width = stmt.column_count();
    let mut out = Vec::new();
    let mut rows = stmt.raw_query();
    while let Some(row) = rows.next().expect("sqlite step") {
        out.push(
            (0..width)
                .map(|i| match row.get_ref(i).expect("value") {
                    rusqlite::types::ValueRef::Null => SqlValue::Null,
                    rusqlite::types::ValueRef::Integer(n) => SqlValue::Integer(n),
                    rusqlite::types::ValueRef::Real(f) => SqlValue::Real(f),
                    rusqlite::types::ValueRef::Text(t) => {
                        SqlValue::Text(std::str::from_utf8(t).expect("utf8").into())
                    }
                    rusqlite::types::ValueRef::Blob(b) => SqlValue::Blob(b.to_vec().into()),
                })
                .collect(),
        );
    }
    out
}

fn setup() -> Lab {
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE t(a INTEGER); INSERT INTO t VALUES (1);
         CREATE TABLE u(b INTEGER);
         CREATE VIEW v AS SELECT a FROM t;",
    );
    lab
}

#[test]
fn held_view_statement_prepared_before_insert_sees_row() {
    let lab = setup();
    for sql in [
        "SELECT a FROM v ORDER BY a",
        "SELECT v.a FROM v JOIN t ON t.a = v.a ORDER BY v.a",
        "WITH c AS (SELECT a FROM t) SELECT a FROM c ORDER BY a",
        "SELECT x FROM (SELECT a AS x FROM t) ORDER BY x",
    ] {
        let mut held = lab.redline.prepare(sql).expect("prepare");
        let mut oracle = lab.sqlite.prepare(sql).expect("sqlite prepare");
        lab.step_both("INSERT INTO t VALUES (2)");
        let expected = sqlite_rows(&mut oracle, &[]);
        assert_eq!(rows_of(&mut held), expected, "{sql}");
        lab.step_both("DELETE FROM t WHERE a = 2");
    }
}

#[test]
fn reset_after_done_rematerializes() {
    let lab = setup();
    let sql = "SELECT count(*) FROM v";
    let mut held = lab.redline.prepare(sql).expect("prepare");
    assert_eq!(rows_of(&mut held), vec![vec![int(1)]]);
    lab.step_both("INSERT INTO t VALUES (2)");
    held.reset().expect("reset");
    assert_eq!(rows_of(&mut held), vec![vec![int(2)]]);
    lab.step_both("INSERT INTO t VALUES (3)");
    held.reset().expect("reset");
    assert_eq!(rows_of(&mut held), vec![vec![int(3)]]);
    lab.assert_rows(sql, &[vec![int(3)]]);
}

#[test]
fn insert_select_join_view_not_cached_stale() {
    let lab = setup();
    for insert in [
        "INSERT INTO u SELECT s.b FROM t JOIN (SELECT a AS b FROM t) s ON 1",
        "INSERT INTO u SELECT s.a FROM t JOIN v s ON 1",
        "WITH s(b) AS (SELECT a FROM t) INSERT INTO u SELECT s.b FROM t JOIN s ON 1",
    ] {
        lab.step_both("DELETE FROM u");
        lab.step_both(insert);
        lab.step_both("INSERT INTO t VALUES (5)");
        // The same SQL text again: a cached template would carry the rows
        // of the first preparation.
        lab.step_both(insert);
        lab.assert_rows(
            "SELECT b FROM u ORDER BY b",
            &[
                vec![int(1)],
                vec![int(1)],
                vec![int(1)],
                vec![int(5)],
                vec![int(5)],
            ],
        );
        lab.step_both("DELETE FROM t WHERE a = 5");
    }
}

#[test]
fn cte_parameter_bound_after_prepare() {
    let lab = setup();
    for sql in [
        "WITH c AS (SELECT ? AS x) SELECT x FROM c",
        "WITH c AS (SELECT :x AS x) SELECT x FROM c",
        "WITH c AS (SELECT a FROM t WHERE a = ?) SELECT a FROM c",
        "WITH c AS (SELECT a FROM t WHERE a = ?1) SELECT c.a FROM c JOIN t ON t.a = c.a",
    ] {
        let mut held = lab.redline.prepare(sql).expect("prepare");
        let mut oracle = lab.sqlite.prepare(sql).expect("sqlite prepare");
        for value in [7, 1, 9] {
            held.reset().expect("reset");
            held.bind_i64(1, value).expect("bind");
            let expected = sqlite_rows(&mut oracle, &[value]);
            assert_eq!(rows_of(&mut held), expected, "{sql} with {value}");
        }
    }
}

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

#[test]
fn create_view_with_parameter_rejected() {
    let lab = setup();
    for sql in [
        "CREATE VIEW pv AS SELECT a FROM t WHERE a = ?",
        "CREATE VIEW pv AS SELECT :x AS x",
    ] {
        let sqlite = lab.sqlite.execute_batch(sql).expect_err("sqlite");
        assert!(
            sqlite
                .to_string()
                .contains("parameters are not allowed in views")
        );
        let err = lab.redline.execute(sql).expect_err(sql);
        assert!(
            err.to_string()
                .contains("parameters are not allowed in views"),
            "{sql}: {err}"
        );
    }
}

#[test]
fn cross_connection_writer_reader_same_handle() {
    let lab = setup();
    let writer = lab.database.connect();
    let sql = "SELECT a FROM v ORDER BY a";
    let mut held = lab.redline.prepare(sql).expect("prepare");
    assert_eq!(rows_of(&mut held), vec![vec![int(1)]]);
    writer.execute("INSERT INTO t VALUES (4)").expect("insert");
    held.reset().expect("reset");
    assert_eq!(rows_of(&mut held), vec![vec![int(1)], vec![int(4)]]);
    let mut fresh = lab.redline.prepare(sql).expect("prepare");
    writer.execute("INSERT INTO t VALUES (6)").expect("insert");
    assert_eq!(
        rows_of(&mut fresh),
        vec![vec![int(1)], vec![int(4)], vec![int(6)]]
    );
}

#[test]
fn a_statement_run_straight_after_its_preparation_reads_its_own_writes() {
    let lab = setup();
    lab.redline.execute("BEGIN").expect("begin");
    let mut held = lab
        .redline
        .prepare("SELECT count(*) FROM v")
        .expect("prepare");
    lab.redline
        .execute("INSERT INTO t VALUES (8)")
        .expect("insert");
    assert_eq!(rows_of(&mut held), vec![vec![int(2)]]);
    lab.redline.execute("ROLLBACK").expect("rollback");
    held.reset().expect("reset");
    assert_eq!(rows_of(&mut held), vec![vec![int(1)]]);
}

/// Declared deviation (docs/sqlite-parity.md): a CTE is materialized when
/// the statement is bound, used or not, so an unused CTE that names a
/// missing table is an error; SQLite ignores the CTE.
#[test]
fn an_unused_cte_naming_a_missing_table_is_an_error() {
    let lab = setup();
    let sql = "WITH unused AS (SELECT * FROM no_such_table) SELECT a FROM t";
    let rows = lab
        .sqlite
        .prepare(sql)
        .map(|mut s| sqlite_rows(&mut s, &[]));
    assert_eq!(rows.expect("sqlite ignores the CTE"), vec![vec![int(1)]]);
    let err = match lab.redline.prepare(sql) {
        Err(err) => err.to_string(),
        Ok(mut stmt) => stmt.step().expect_err("redline binds the CTE").to_string(),
    };
    assert!(err.contains("object not found"), "{err}");
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
