//! A statement SQLite rejects fails in RedlineDB with SQLite's own words.
//!
//! Each case runs its setup and then the rejected statement against the
//! bundled SQLite (rusqlite) and against RedlineDB in the SQLite dialect.
//! RedlineDB's error must contain the message SQLite gives, so the wording is
//! checked against SQLite itself rather than against strings typed here. The
//! statements are the ones the sqlite_parity corpus holds to sqlite3 3.53.1's
//! text (case ids in the comments).

use std::sync::Arc;

use redlinedb_sql::{Connection, Database, DbOptions, Dialect, Step};

fn open_redline() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let options = DbOptions {
        dialect: Some(Dialect::Sqlite),
        ..DbOptions::default()
    };
    let db = Database::create(dir.path().join("wording.db"), options).expect("create db");
    (dir, db.connect())
}

/// The error preparing or stepping `sql` raises in RedlineDB.
fn redline_error(conn: &Arc<Connection>, sql: &str) -> String {
    let mut stmt = match conn.prepare(sql) {
        Ok(stmt) => stmt,
        Err(err) => return err.to_string(),
    };
    loop {
        match stmt.step() {
            Ok(Step::Row) => continue,
            Ok(Step::Done) => panic!("RedlineDB accepted {sql}"),
            Err(err) => return err.to_string(),
        }
    }
}

/// The message SQLite gives for `sql`, preparing or stepping it.
fn sqlite_error(conn: &rusqlite::Connection, sql: &str) -> String {
    let message = |err: rusqlite::Error| match err {
        rusqlite::Error::SqliteFailure(_, Some(message))
        | rusqlite::Error::SqlInputError { msg: message, .. } => message,
        other => panic!("SQLite failed {sql} without a message: {other:?}"),
    };
    let mut stmt = match conn.prepare(sql) {
        Ok(stmt) => stmt,
        Err(err) => return message(err),
    };
    let mut rows = stmt.raw_query();
    loop {
        match rows.next() {
            Ok(Some(_)) => continue,
            Ok(None) => panic!("SQLite accepted {sql}"),
            Err(err) => return message(err),
        }
    }
}

/// Run `setup` in both engines, then check that `sql` fails in RedlineDB
/// with the message SQLite gives for it.
fn assert_sqlite_wording(setup: &[&str], sql: &str) {
    let sqlite = rusqlite::Connection::open_in_memory().expect("open sqlite");
    let (_dir, redline) = open_redline();
    for statement in setup {
        sqlite.execute_batch(statement).expect("sqlite setup");
        redline.execute(statement).expect("redline setup");
    }
    let expected = sqlite_error(&sqlite, sql);
    let actual = redline_error(&redline, sql);
    assert!(
        actual.contains(&expected),
        "{sql}: RedlineDB said {actual:?}, SQLite said {expected:?}"
    );
}

#[test]
fn transaction_control_outside_its_state() {
    // 10299, 10565
    assert_sqlite_wording(&["BEGIN"], "BEGIN");
    // 10300, 10567
    assert_sqlite_wording(&[], "COMMIT");
    // 10301, 10566
    assert_sqlite_wording(&[], "ROLLBACK");
    // 10560
    assert_sqlite_wording(&["SAVEPOINT real"], "RELEASE bogus");
    assert_sqlite_wording(&["BEGIN"], "ROLLBACK TO bogus");
}

#[test]
fn missing_catalog_objects_are_named() {
    // 10554
    assert_sqlite_wording(&[], "SELECT * FROM missing");
    assert_sqlite_wording(&[], "SELECT * FROM main.missing");
    assert_sqlite_wording(&[], "INSERT INTO missing VALUES (1)");
    assert_sqlite_wording(&[], "DROP TABLE missing");
    assert_sqlite_wording(&[], "ALTER TABLE missing RENAME TO other");
    // 10561
    assert_sqlite_wording(&[], "DROP TRIGGER bogus");
    assert_sqlite_wording(&[], "DROP VIEW missing");
    assert_sqlite_wording(&[], "DROP INDEX missing");
    // 10584
    assert_sqlite_wording(
        &[
            "PRAGMA foreign_keys=ON",
            "CREATE TABLE c(y INTEGER REFERENCES nonexistent_tbl(id))",
        ],
        "INSERT INTO c VALUES (1)",
    );
}

#[test]
fn schema_tables_and_views_refuse_table_statements() {
    // 10579, 10580
    assert_sqlite_wording(&[], "INSERT INTO sqlite_master VALUES (1,2,3,4,5)");
    assert_sqlite_wording(&[], "DROP TABLE sqlite_master");
    assert_sqlite_wording(&[], "ALTER TABLE sqlite_master RENAME TO other");
    // 10572, 10573
    assert_sqlite_wording(&["CREATE VIEW v AS SELECT 1"], "ALTER TABLE v RENAME TO w");
    assert_sqlite_wording(&["CREATE VIEW v AS SELECT 1"], "DROP TABLE v");
    assert_sqlite_wording(&["CREATE TABLE t(x)"], "DROP VIEW t");
}

#[test]
fn duplicate_names_name_the_existing_object() {
    // 10581, 10582, 10583
    assert_sqlite_wording(&[], "CREATE TABLE t(x, x)");
    assert_sqlite_wording(&["CREATE TABLE t(x)"], "CREATE TABLE t(y)");
    assert_sqlite_wording(&["CREATE VIEW v AS SELECT 1"], "CREATE VIEW v AS SELECT 2");
    assert_sqlite_wording(&["CREATE TABLE t(x)"], "CREATE VIEW t AS SELECT 1");
    // 10384
    assert_sqlite_wording(
        &["ATTACH DATABASE ':memory:' AS aux"],
        "ATTACH DATABASE ':memory:' AS aux",
    );
}

#[test]
fn constraint_failures_name_the_columns() {
    // 10547, 10548
    assert_sqlite_wording(
        &["CREATE TABLE t(x UNIQUE)", "INSERT INTO t VALUES (1)"],
        "INSERT INTO t VALUES (1)",
    );
    assert_sqlite_wording(
        &[
            "CREATE TABLE t(x INTEGER PRIMARY KEY)",
            "INSERT INTO t VALUES (1)",
        ],
        "INSERT INTO t VALUES (1)",
    );
    assert_sqlite_wording(
        &[
            "CREATE TABLE t(a, b, UNIQUE(a, b))",
            "INSERT INTO t VALUES (1, 2)",
        ],
        "INSERT INTO t VALUES (1, 2)",
    );
    // 10253, 10568
    assert_sqlite_wording(
        &["CREATE TABLE t(x)", "PRAGMA query_only=ON"],
        "INSERT INTO t VALUES (1)",
    );
}

#[test]
fn syntax_errors_name_the_token() {
    // 10562, 10563, 10564
    assert_sqlite_wording(&[], "SELECT x +");
    assert_sqlite_wording(&[], "selct 1");
    assert_sqlite_wording(&[], "SELECT (1");
    // 10078
    assert_sqlite_wording(
        &[],
        "CREATE TABLE t(a INTEGER, b TEXT, PRIMARY KEY(a,b) AUTOINCREMENT)",
    );
    // 00219, 00220
    let setup = ["CREATE TABLE t(id INT PRIMARY KEY, v INT)"];
    assert_sqlite_wording(&setup, "UPDATE t SET v=v+100 ORDER BY id DESC LIMIT 1");
    assert_sqlite_wording(&setup, "DELETE FROM t ORDER BY id LIMIT 1");
    assert_sqlite_wording(&setup, "DELETE FROM t LIMIT 1");
    // 10495, 10496, 10525
    let setup = ["CREATE TABLE t(x INTEGER)", "INSERT INTO t VALUES (1),(2)"];
    assert_sqlite_wording(&setup, "SELECT 1 = ANY (SELECT x FROM t)");
    assert_sqlite_wording(&setup, "SELECT 99 > ALL (SELECT x FROM t)");
    assert_sqlite_wording(&setup, "SELECT x FROM t GROUP BY GROUPING SETS((x))");
    // 10545
    assert_sqlite_wording(
        &[],
        "WITH v(x) AS (VALUES (1),(2)) \
         SELECT percentile_cont(0.5) WITHIN GROUP (ORDER BY x) FROM v",
    );
}

#[test]
fn functions_and_subqueries_misused() {
    // 10557, 10524
    assert_sqlite_wording(&[], "SELECT bogus_func(1)");
    assert_sqlite_wording(
        &[
            "CREATE TABLE t(a TEXT, b TEXT, x INTEGER)",
            "INSERT INTO t VALUES ('x','1',10)",
        ],
        "SELECT a, b, sum(x) FROM t GROUP BY ROLLUP(a, b)",
    );
    // 10586
    assert_sqlite_wording(&[], "SELECT sum(sum(x)) FROM (SELECT 1 AS x)");
    // 10502, 10585
    assert_sqlite_wording(&[], "SELECT (SELECT 1, 2)");
    assert_sqlite_wording(&[], "SELECT 1 IN (SELECT 1, 2)");
    // 10578
    assert_sqlite_wording(&[], "SELECT 1 AS a UNION SELECT 2 ORDER BY b");
    // 10504
    assert_sqlite_wording(
        &["CREATE TABLE u(x INTEGER)"],
        "CREATE TABLE t(x INTEGER CHECK(x IN (SELECT x FROM u)))",
    );
}

#[test]
fn raise_is_refused_outside_a_trigger() {
    // 10570
    assert_sqlite_wording(&[], "SELECT RAISE(ABORT, 'no')");
    assert_sqlite_wording(&[], "SELECT RAISE(IGNORE)");
}

#[test]
fn table_definitions_sqlite_refuses() {
    // 10097, 10098
    assert_sqlite_wording(&[], "CREATE TABLE t(a VARCHAR) STRICT");
    assert_sqlite_wording(&[], "CREATE TABLE t(a NUMERIC) STRICT");
    assert_sqlite_wording(&[], "CREATE TABLE t(a) STRICT");
    // 10418, 10425
    assert_sqlite_wording(
        &[
            "CREATE TABLE t(a INTEGER, b TEXT, c REAL)",
            "CREATE INDEX idx_t_b ON t(b)",
        ],
        "ALTER TABLE t DROP COLUMN b",
    );
    assert_sqlite_wording(
        &["CREATE TABLE t(a INTEGER)"],
        "ALTER TABLE t DROP COLUMN a",
    );
    assert_sqlite_wording(&["CREATE TABLE t(a, b)"], "ALTER TABLE t DROP COLUMN zz");
}

#[test]
fn unattached_qualifiers_are_missing_tables() {
    // 10389
    let sqlite = rusqlite::Connection::open_in_memory().expect("open sqlite");
    let (_dir, redline) = open_redline();
    for statement in [
        "ATTACH DATABASE ':memory:' AS aux",
        "CREATE TABLE aux.t(x)",
        "DETACH aux",
    ] {
        sqlite.execute_batch(statement).expect("sqlite setup");
        redline.execute(statement).expect("redline setup");
    }
    let sql = "SELECT * FROM aux.t";
    let expected = sqlite_error(&sqlite, sql);
    let actual = redline_error(&redline, sql);
    assert!(actual.contains(&expected), "{actual:?} vs {expected:?}");
    let expected = sqlite_error(&sqlite, "DETACH aux");
    let actual = redline_error(&redline, "DETACH aux");
    assert!(actual.contains(&expected), "{actual:?} vs {expected:?}");
}
