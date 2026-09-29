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

#[path = "launch_view_freshness/parameters.rs"]
mod parameters;

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
    assert!(err.contains("no such table: no_such_table"), "{err}");
}

/// A data-modifying CTE writes while its statement is bound (Postgres
/// dialect). Binding it again at execution would write twice, so such a
/// statement keeps the rows of its preparation and writes once per
/// preparation (beyond-SQLite cases 20108 and 20109). A reset and re-step
/// returns those rows again without writing again; PostgreSQL would run the
/// UPDATE again, a limit listed under "Known defects at release" in
/// docs/launch/v5.0.0-evidence.md.
#[test]
fn a_data_modifying_cte_writes_once() {
    use redlinedb_sql::{Database, DbOptions, Dialect};
    let dir = tempfile::tempdir().expect("tempdir");
    let options = DbOptions {
        dialect: Some(Dialect::PostgresSubset),
        ..DbOptions::default()
    };
    let db = Database::create(dir.path().join("pg.db"), options).expect("db");
    let conn = db.connect();
    conn.execute("CREATE TABLE d(id int, n int)")
        .expect("create");
    conn.execute("INSERT INTO d VALUES (1, 10), (2, 20)")
        .expect("insert");
    let mut stmt = conn
        .prepare("WITH upd AS (UPDATE d SET n = n * 2 RETURNING *) SELECT * FROM upd ORDER BY id")
        .expect("prepare");
    let expected = vec![vec![int(1), int(20)], vec![int(2), int(40)]];
    assert_eq!(rows_of(&mut stmt), expected);
    stmt.reset().expect("reset");
    assert_eq!(rows_of(&mut stmt), expected);
    drop(stmt);
    let mut check = conn.prepare("SELECT n FROM d ORDER BY id").expect("check");
    assert_eq!(rows_of(&mut check), vec![vec![int(20)], vec![int(40)]]);
    let mut count = conn
        .prepare("WITH del AS (DELETE FROM d WHERE id > 1 RETURNING *) SELECT count(*) FROM del")
        .expect("prepare delete");
    assert_eq!(rows_of(&mut count), vec![vec![int(1)]]);
}
