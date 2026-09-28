//! PG-09: the SQL dialect belongs to a database, not to the process.
//!
//! `REDLINEDB_RESULT_DIALECT` used to be read on every boolean, comparison
//! and truth test, so one process could only speak one dialect and a
//! `set_var` after open changed the answers of a database already in use.
//! The dialect is now `DbOptions::dialect`, resolved once when the database
//! opens (the environment is only the fallback when the option is `None`).
//!
//! Only `the_environment_is_read_once_at_open` touches the environment.

use std::sync::Arc;
use std::thread;

use redlinedb_sql::{Connection, Database, DbOptions, Dialect, SqlValue, Step};

fn open(dir: &tempfile::TempDir, name: &str, dialect: Option<Dialect>) -> Arc<Connection> {
    let options = DbOptions {
        dialect,
        ..DbOptions::default()
    };
    Database::create(dir.path().join(name), options)
        .expect("create db")
        .connect()
}

fn scalar(conn: &Arc<Connection>, sql: &str) -> SqlValue {
    let mut stmt = conn.prepare(sql).expect("prepare");
    assert!(matches!(stmt.step().expect("step"), Step::Row), "{sql}");
    stmt.column_value(0).expect("column").clone()
}

fn pg_true() -> SqlValue {
    SqlValue::Text(Arc::from("t"))
}

#[test]
fn connections_with_different_dialects_run_side_by_side() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pg = open(&dir, "pg.db", Some(Dialect::PostgresSubset));
    let lite = open(&dir, "lite.db", Some(Dialect::Sqlite));
    assert_eq!(pg.dialect(), Dialect::PostgresSubset);
    assert_eq!(lite.dialect(), Dialect::Sqlite);

    // Interleave two open statements on one thread: each step must see its
    // own connection's dialect, not the one the previous step left behind.
    let mut on_pg = pg.prepare("SELECT 1=1").expect("prepare pg");
    let mut on_lite = lite.prepare("SELECT 1=1").expect("prepare sqlite");
    assert!(matches!(on_pg.step().expect("step pg"), Step::Row));
    assert!(matches!(on_lite.step().expect("step sqlite"), Step::Row));
    assert_eq!(on_pg.column_value(0).expect("pg").clone(), pg_true());
    assert_eq!(
        on_lite.column_value(0).expect("sqlite").clone(),
        SqlValue::Integer(1)
    );

    let workers = [(pg, pg_true()), (lite, SqlValue::Integer(1))]
        .into_iter()
        .map(|(conn, want)| {
            thread::spawn(move || {
                for i in 0..200 {
                    // A fresh SQL text each round so no cached template hides
                    // a parse-time dialect decision.
                    let sql = format!("SELECT {i} = {i}");
                    assert_eq!(scalar(&conn, &sql), want, "{sql}");
                    assert_eq!(scalar(&conn, "SELECT 2 > 1"), want);
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().expect("worker");
    }
}

#[test]
fn parse_time_rewrites_follow_the_connection_dialect() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pg = open(&dir, "pg.db", Some(Dialect::PostgresSubset));
    let lite = open(&dir, "lite.db", Some(Dialect::Sqlite));
    // Postgres accepts NULLS FIRST on an index key; SQLite's shell rejects it.
    pg.execute("CREATE TABLE t(x INTEGER)").expect("pg table");
    pg.execute("CREATE INDEX t_x ON t(x NULLS FIRST)")
        .expect("pg accepts NULLS FIRST");
    lite.execute("CREATE TABLE t(x INTEGER)")
        .expect("sqlite table");
    let err = lite
        .execute("CREATE INDEX t_x ON t(x NULLS FIRST)")
        .expect_err("sqlite rejects NULLS FIRST");
    assert!(err.to_string().contains("NULLS FIRST"), "{err}");
}

#[test]
fn a_large_sort_keeps_the_dialect() {
    // 70,000 rows crosses the parallel-sort threshold, so the ORDER BY runs
    // on rayon workers while the projection stays on the statement thread.
    let dir = tempfile::tempdir().expect("tempdir");
    let pg = open(&dir, "pg.db", Some(Dialect::PostgresSubset));
    pg.execute("CREATE TABLE big(x INTEGER)").expect("table");
    pg.execute("INSERT INTO big SELECT value FROM generate_series(1, 70000) ORDER BY random()")
        .expect("fill");
    let mut stmt = pg
        .prepare("SELECT x, x % 2 = 0 FROM big ORDER BY x DESC")
        .expect("prepare");
    let mut rows = 0i64;
    while let Step::Row = stmt.step().expect("step") {
        let x = match stmt.column_value(0).expect("x") {
            SqlValue::Integer(x) => *x,
            other => panic!("x is {other:?}"),
        };
        assert_eq!(x, 70000 - rows, "descending order");
        let even = if x % 2 == 0 { "t" } else { "f" };
        assert_eq!(
            stmt.column_value(1).expect("flag").clone(),
            SqlValue::Text(Arc::from(even))
        );
        rows += 1;
    }
    assert_eq!(rows, 70000);
}

#[test]
fn the_environment_is_read_once_at_open() {
    let dir = tempfile::tempdir().expect("tempdir");
    // SAFETY: this is the only test in this binary that touches the
    // environment, and every other test passes an explicit dialect.
    unsafe { std::env::set_var(Dialect::ENV_VAR, "postgres") };
    let from_env_pg = open(&dir, "env_pg.db", None);
    unsafe { std::env::remove_var(Dialect::ENV_VAR) };
    let from_env_lite = open(&dir, "env_lite.db", None);

    // The variable is gone, but the first database resolved it at open.
    assert_eq!(scalar(&from_env_pg, "SELECT 1=1"), pg_true());
    assert_eq!(scalar(&from_env_lite, "SELECT 1=1"), SqlValue::Integer(1));

    // Setting it again changes neither open database.
    unsafe { std::env::set_var(Dialect::ENV_VAR, "postgres") };
    assert_eq!(scalar(&from_env_lite, "SELECT 3=3"), SqlValue::Integer(1));
    unsafe { std::env::set_var(Dialect::ENV_VAR, "sqlite") };
    assert_eq!(scalar(&from_env_pg, "SELECT 3=3"), pg_true());
    unsafe { std::env::remove_var(Dialect::ENV_VAR) };
}
