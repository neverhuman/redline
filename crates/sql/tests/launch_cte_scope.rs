//! Q5-09: CTE scopes and the rows behind CTEs, derived tables and views
//! belong to the statement that bound them.
//!
//! The binder materializes CTEs, derived tables and views into rows kept in
//! a per-thread registry under synthetic relation ids, and resolves a CTE
//! name that no scope binds through a per-thread "permanent" map. The ids
//! restarted at 1 on every WITH, the map outlived its statement, and a
//! failed CTE bind left its scope pushed. So a nested WITH overwrote the
//! outer CTE's rows (`11|22` came back `22|22`), a CTE name shadowed the
//! real table in every later statement on the thread (an `INSERT ... SELECT`
//! persisted CTE rows, a `DELETE ... IN` deleted nothing), two held
//! statements read each other's rows, and one database's CTE answered for
//! another's table. Every expectation here is checked against bundled SQLite.

#[path = "launch_wrong_answer/lab.rs"]
#[allow(dead_code)]
mod lab;

use lab::{Lab, int};
use redlinedb_sql::{Database, DbOptions, SqlValue, Step};

fn rows_of(stmt: &mut redlinedb_sql::Statement) -> Vec<Vec<SqlValue>> {
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

#[test]
fn nested_with_in_derived_table_keeps_outer_rows() {
    let lab = Lab::new();
    lab.exec_both("CREATE TABLE t(a INTEGER); INSERT INTO t VALUES (1);");
    lab.assert_rows(
        "WITH a(x) AS (SELECT 11) SELECT a.x, s.y FROM a \
         JOIN (WITH b(y) AS (SELECT 22) SELECT y FROM b) s ON 1",
        &[vec![int(11), int(22)]],
    );
    lab.assert_rows(
        "WITH a(x) AS (SELECT 11) SELECT a.x, s.y FROM a \
         JOIN (SELECT y FROM (WITH b(y) AS (SELECT 22) SELECT y FROM b)) s ON 1",
        &[vec![int(11), int(22)]],
    );
    lab.assert_rows(
        "WITH a(x) AS (SELECT 11) SELECT a.x, t.a FROM a \
         JOIN t ON t.a IN (WITH b(y) AS (SELECT 1) SELECT y FROM b)",
        &[vec![int(11), int(1)]],
    );
}

#[test]
fn view_with_cte_keeps_outer_cte() {
    let lab = Lab::new();
    lab.exec_both("CREATE VIEW v AS WITH b(y) AS (SELECT 22) SELECT y FROM b;");
    lab.assert_rows(
        "WITH a(x) AS (SELECT 11) SELECT a.x, v.y FROM a JOIN v ON 1",
        &[vec![int(11), int(22)]],
    );
    lab.assert_rows(
        "WITH a(x) AS (SELECT 11), c(z) AS (SELECT 33) \
         SELECT a.x, v.y, c.z FROM a JOIN v ON 1 JOIN c ON 1",
        &[vec![int(11), int(22), int(33)]],
    );
}

#[test]
fn cte_name_does_not_shadow_table_in_later_statement() {
    let lab = Lab::new();
    lab.exec_both("CREATE TABLE t(a INTEGER); INSERT INTO t VALUES (1);");
    lab.assert_rows("WITH t(a) AS (SELECT 99) SELECT a FROM t", &[vec![int(99)]]);
    lab.assert_rows("SELECT a FROM t", &[vec![int(1)]]);
    lab.assert_rows("SELECT (SELECT a FROM t)", &[vec![int(1)]]);
    lab.assert_rows(
        "SELECT EXISTS (SELECT 1 FROM t WHERE a = 99)",
        &[vec![int(0)]],
    );
    lab.assert_rows("SELECT 1 IN (SELECT a FROM t)", &[vec![int(1)]]);
    lab.assert_rows("SELECT x FROM (SELECT a AS x FROM t)", &[vec![int(1)]]);

    lab.assert_rows("WITH t(a) AS (SELECT 99) SELECT a FROM t", &[vec![int(99)]]);
    lab.step_both("INSERT INTO t SELECT a + 1 FROM t");
    lab.assert_rows("SELECT a FROM t ORDER BY a", &[vec![int(1)], vec![int(2)]]);

    lab.assert_rows("WITH t(a) AS (SELECT 99) SELECT a FROM t", &[vec![int(99)]]);
    lab.step_both("DELETE FROM t WHERE a IN (SELECT a FROM t)");
    lab.assert_rows("SELECT count(*) FROM t", &[vec![int(0)]]);
}

#[test]
fn failed_cte_bind_pops_scope() {
    let lab = Lab::new();
    lab.exec_both("CREATE TABLE t(a INTEGER); INSERT INTO t VALUES (1);");
    // The first CTE binds; the second fails, after the first CTE's scope
    // was pushed. (Both engines refuse; RedlineDB's message differs, which
    // is not what this test is about.)
    let failing = "WITH t(a) AS (SELECT 99), broken AS (SELECT * FROM no_such_table) \
                   SELECT a FROM t, broken";
    assert!(lab.sqlite.prepare(failing).is_err());
    assert!(lab.redline.prepare(failing).is_err());
    // A later WITH of its own must not see the half-bound scope either.
    lab.assert_rows(
        "WITH z(q) AS (SELECT 5) SELECT a, q FROM t, z",
        &[vec![int(1), int(5)]],
    );
    lab.assert_rows("SELECT a FROM t", &[vec![int(1)]]);
    lab.assert_rows(
        "SELECT a FROM (SELECT a FROM t) WHERE a = 1",
        &[vec![int(1)]],
    );
}

#[test]
fn two_held_cte_join_statements_interleaved() {
    let lab = Lab::new();
    lab.exec_both("CREATE TABLE t(a INTEGER); INSERT INTO t VALUES (1);");
    let first_sql = "WITH c(v) AS (SELECT 11) SELECT t.a, c.v FROM t JOIN c ON 1";
    let second_sql = "WITH c(v) AS (SELECT 22) SELECT t.a, c.v FROM t JOIN c ON 1";
    let mut first = lab.redline.prepare(first_sql).expect("prepare first");
    let mut second = lab.redline.prepare(second_sql).expect("prepare second");
    assert_eq!(rows_of(&mut first), vec![vec![int(1), int(11)]]);
    assert_eq!(rows_of(&mut second), vec![vec![int(1), int(22)]]);
    first.reset().expect("reset");
    assert_eq!(rows_of(&mut first), vec![vec![int(1), int(11)]]);
    drop(second);
    first.reset().expect("reset");
    assert_eq!(rows_of(&mut first), vec![vec![int(1), int(11)]]);
    lab.assert_rows(first_sql, &[vec![int(1), int(11)]]);
    lab.assert_rows(second_sql, &[vec![int(1), int(22)]]);
}

#[test]
fn two_databases_same_thread_isolated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let one = Database::create(dir.path().join("one.db"), DbOptions::default()).expect("one");
    let two = Database::create(dir.path().join("two.db"), DbOptions::default()).expect("two");
    let a = one.connect();
    let b = two.connect();
    b.execute("CREATE TABLE t(a INTEGER); INSERT INTO t VALUES (1);")
        .expect("setup two");
    let mut with = a
        .prepare("WITH t(a) AS (SELECT 99) SELECT a FROM t")
        .expect("prepare on one");
    assert_eq!(rows_of(&mut with), vec![vec![int(99)]]);
    let mut plain = b.prepare("SELECT a FROM t").expect("prepare on two");
    assert_eq!(rows_of(&mut plain), vec![vec![int(1)]]);
    // Database one has no table t, and database two's WITH is gone.
    let outcome = match a.prepare("SELECT a FROM t") {
        Err(err) => Err(err.to_string()),
        Ok(mut stmt) => match stmt.step() {
            Err(err) => Err(err.to_string()),
            Ok(_) => Ok(stmt.column_value(0).map(|v| v.clone())),
        },
    };
    assert!(outcome.is_err(), "database one resolved t: {outcome:?}");
}

#[test]
fn view_and_trigger_bodies_do_not_see_the_callers_ctes() {
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE t(a INTEGER); INSERT INTO t VALUES (1);
         CREATE VIEW v AS SELECT a FROM t;
         CREATE TABLE log(x); CREATE TABLE audit(y);
         CREATE TRIGGER tr AFTER INSERT ON log BEGIN INSERT INTO audit SELECT a FROM t; END;",
    );
    // A view body names the real table t, whatever the query around it
    // calls a CTE.
    lab.assert_rows("WITH t(a) AS (SELECT 99) SELECT a FROM v", &[vec![int(1)]]);
    lab.assert_rows(
        "WITH t(a) AS (SELECT 99) SELECT v.a, t.a FROM v JOIN t ON 1",
        &[vec![int(1), int(99)]],
    );
    lab.assert_rows(
        "WITH t(a) AS (SELECT 99) SELECT (SELECT a FROM v), a FROM t",
        &[vec![int(1), int(99)]],
    );
    // So does a trigger body. (INSERT ... VALUES: RedlineDB does not fire
    // AFTER INSERT triggers for INSERT ... SELECT, a separate defect.)
    lab.step_both("WITH t(a) AS (SELECT 99) INSERT INTO log VALUES ((SELECT a FROM t))");
    lab.assert_rows("SELECT x FROM log", &[vec![int(99)]]);
    lab.assert_rows("SELECT y FROM audit", &[vec![int(1)]]);
}
