//! PG-03: U+E000 is an ordinary character outside Postgres `::citext`.
//!
//! `::citext` casts mark their value with a leading U+E000, and the value
//! comparator used to treat ANY text with that prefix as case-insensitive,
//! in every dialect and process. In default SQLite mode `char(57344)||'A'`
//! then equalled `'a'`, a UNIQUE column rejected it next to `'a'`, and an
//! indexed and an unindexed `WHERE x='a'` counted different rows. The
//! marker now means "compare without case" only in a Postgres-dialect
//! statement after `CREATE EXTENSION citext`; everywhere else it compares
//! as the bytes it is, as in SQLite.
//!
//! No test here touches the environment; dialects are explicit options.

#[path = "launch_wrong_answer/lab.rs"]
mod lab;

use std::sync::Arc;

use lab::{Lab, int, text};
use redlinedb_sql::{Connection, Database, DbOptions, Dialect, SqlValue, Step};

const MARKED_UPPER: &str = "\u{E000}A";
const MARKED_LOWER: &str = "\u{E000}a";

/// A table holding U+E000 + 'A', 'A' and 'a', with or without a UNIQUE
/// index, on both engines.
fn marked_lab(unique: bool) -> Lab {
    let lab = Lab::new();
    let constraint = if unique { " UNIQUE" } else { "" };
    lab.exec_both(&format!(
        "CREATE TABLE t(x TEXT{constraint});\
         INSERT INTO t VALUES(char(57344)||'A');\
         INSERT INTO t VALUES('A');\
         INSERT INTO t VALUES('a');"
    ));
    lab
}

#[test]
fn equality_treats_the_marker_as_a_character() {
    let lab = Lab::new();
    lab.assert_rows(
        "SELECT char(57344)||'A' = 'a', char(57344)||'A' = 'A', \
         char(57344)||'A' = char(57344)||'A', char(57344)||'A' = char(57344)||'a', \
         char(57344)||'a' = 'A' COLLATE BINARY",
        &[vec![int(0), int(0), int(1), int(0), int(0)]],
    );
    lab.assert_rows(
        "SELECT char(57344)||'A' < 'a', 'z' < char(57344)||'A'",
        &[vec![int(0), int(1)]],
    );
}

#[test]
fn distinct_group_by_and_order_by_see_three_values() {
    let lab = marked_lab(false);
    lab.assert_same("SELECT DISTINCT x FROM t", false);
    lab.assert_rows("SELECT count(DISTINCT x) FROM t", &[vec![int(3)]]);
    lab.assert_same("SELECT x, count(*) FROM t GROUP BY x", false);
    lab.assert_rows(
        "SELECT x FROM t ORDER BY x",
        &[vec![text("A")], vec![text("a")], vec![text(MARKED_UPPER)]],
    );
    lab.assert_rows(
        "SELECT x FROM t ORDER BY x DESC",
        &[vec![text(MARKED_UPPER)], vec![text("a")], vec![text("A")]],
    );
    lab.assert_rows(
        "SELECT min(x), max(x) FROM t",
        &[vec![text("A"), text(MARKED_UPPER)]],
    );
}

#[test]
fn unique_accepts_the_marked_spelling_next_to_the_plain_ones() {
    // The three rows above went in without a false UNIQUE violation.
    let lab = marked_lab(true);
    lab.step_both("INSERT INTO t VALUES(char(57344)||'a')");
    // A true duplicate is still refused on both engines.
    lab.step_both("INSERT INTO t VALUES(char(57344)||'A')");
    lab.step_both("INSERT INTO t VALUES('a')");
    lab.assert_rows("SELECT count(*) FROM t", &[vec![int(4)]]);
}

#[test]
fn indexed_and_scanned_lookups_agree() {
    let lab = marked_lab(false);
    lab.exec_both("CREATE INDEX t_x ON t(x)");
    for probe in ["'a'", "'A'", "char(57344)||'A'", "char(57344)||'a'"] {
        lab.indexed_vs_scan(
            "t",
            "t_x",
            &format!("SELECT count(*) FROM {{access}} WHERE x = {probe}"),
        );
        lab.indexed_vs_scan(
            "t",
            "t_x",
            &format!("SELECT x FROM {{access}} WHERE x = {probe}"),
        );
        lab.indexed_vs_scan(
            "t",
            "t_x",
            &format!("SELECT x FROM {{access}} WHERE x >= {probe} ORDER BY x"),
        );
    }
    lab.assert_rows(
        "SELECT count(*) FROM t INDEXED BY t_x WHERE x = 'a'",
        &[vec![int(1)]],
    );
}

#[test]
fn bound_marked_text_is_plain_text() {
    let lab = marked_lab(false);
    for value in [MARKED_UPPER, MARKED_LOWER, "a"] {
        lab.assert_same_bound("SELECT count(*) FROM t WHERE x = ?1", &[text(value)], true);
    }
}

fn open(dir: &tempfile::TempDir, name: &str, dialect: Dialect) -> Arc<Connection> {
    let options = DbOptions {
        dialect: Some(dialect),
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

fn pg_bool(yes: bool) -> SqlValue {
    SqlValue::Text(Arc::from(if yes { "t" } else { "f" }))
}

#[test]
fn citext_in_a_postgres_connection_leaves_sqlite_connections_alone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pg = open(&dir, "pg.db", Dialect::PostgresSubset);
    let lite = open(&dir, "lite.db", Dialect::Sqlite);
    pg.execute("CREATE EXTENSION IF NOT EXISTS citext")
        .expect("citext");
    assert_eq!(
        scalar(&pg, "SELECT 'ABC'::citext = 'abc'::citext"),
        pg_bool(true)
    );
    assert_eq!(
        scalar(&pg, "SELECT 'ABC'::citext = 'abd'::citext"),
        pg_bool(false)
    );
    // Citext is now enabled in this process, but only for Postgres-dialect
    // statements.
    assert_eq!(
        scalar(&lite, "SELECT char(57344)||'A' = char(57344)||'a'"),
        SqlValue::Integer(0)
    );
    lite.execute("CREATE TABLE t(x TEXT UNIQUE)")
        .expect("table");
    lite.execute("INSERT INTO t VALUES(char(57344)||'A')")
        .expect("insert marked");
    lite.execute("INSERT INTO t VALUES(char(57344)||'a')")
        .expect("a different value, not a duplicate");
    assert_eq!(
        scalar(&lite, "SELECT count(DISTINCT x) FROM t"),
        SqlValue::Integer(2)
    );
}

#[test]
fn postgres_citext_columns_are_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pg = open(&dir, "pg.db", Dialect::PostgresSubset);
    pg.execute("CREATE EXTENSION IF NOT EXISTS citext")
        .expect("citext");
    let err = pg
        .execute("CREATE TABLE t(id INTEGER, email citext)")
        .expect_err("a citext column would compare with case");
    let message = err.to_string();
    assert!(
        message.starts_with("unsupported capability: citext column:"),
        "{message}"
    );
    pg.execute("CREATE TABLE t(id INTEGER, email TEXT)")
        .expect("plain table");
    let err = pg
        .execute("ALTER TABLE t ADD COLUMN alias CITEXT")
        .expect_err("ALTER TABLE ADD COLUMN citext is refused too");
    assert!(err.to_string().contains("citext column"), "{err}");
    // Casts stay available.
    pg.execute("INSERT INTO t VALUES(1, 'Ann@Example.com')")
        .expect("insert");
    assert_eq!(
        scalar(
            &pg,
            "SELECT count(*) FROM t WHERE email::citext = 'ann@example.com'::citext"
        ),
        SqlValue::Integer(1)
    );

    // The SQLite dialect has no citext type: the name is a declared type
    // with TEXT affinity, as SQLite reads it.
    let lite = open(&dir, "lite.db", Dialect::Sqlite);
    lite.execute("CREATE TABLE t(x citext)")
        .expect("SQLite accepts any type name");
}

#[test]
fn a_large_citext_sort_on_worker_threads_ignores_case() {
    // 70,000 rows cross the parallel-sort threshold, so the comparisons run
    // on rayon workers, which must see the statement's Postgres dialect.
    let dir = tempfile::tempdir().expect("tempdir");
    let pg = open(&dir, "pg.db", Dialect::PostgresSubset);
    pg.execute("CREATE EXTENSION IF NOT EXISTS citext")
        .expect("citext");
    pg.execute("CREATE TABLE big(i INTEGER)").expect("table");
    pg.execute("INSERT INTO big SELECT value FROM generate_series(1, 70000)")
        .expect("fill");
    // Without case 'a' < 'B'; byte order would put 'B' (0x42) first.
    let mut stmt = pg
        .prepare(
            "SELECT v FROM (SELECT (CASE WHEN i % 2 = 0 THEN 'B' ELSE 'a' END)::citext AS v \
             FROM big) ORDER BY v",
        )
        .expect("prepare");
    let mut seen = Vec::with_capacity(70_000);
    while let Step::Row = stmt.step().expect("step") {
        seen.push(match stmt.column_value(0).expect("v") {
            SqlValue::Text(v) => v.trim_start_matches('\u{E000}').to_owned(),
            other => panic!("v is {other:?}"),
        });
    }
    assert_eq!(seen.len(), 70_000);
    let first_b = seen.iter().position(|v| v == "B").expect("some B");
    assert_eq!(first_b, 35_000, "every 'a' sorts before every 'B'");
    assert!(seen[first_b..].iter().all(|v| v == "B"));
}
