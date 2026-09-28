//! Parity case 10340 — NOCASE collated UNIQUE index as UPSERT conflict target.
//!
//! SQLite treats `CREATE UNIQUE INDEX … (col COLLATE NOCASE)` as a
//! case-insensitive uniqueness constraint. An `ON CONFLICT(col COLLATE NOCASE)`
//! UPSERT arm must fire when the incoming value matches an existing value under
//! NOCASE comparison, even if the raw bytes differ.
use std::sync::Arc;

use redlinedb_sql::{Connection, Database, DbOptions, Step};
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("nocase_unique.db");
    let db = Database::create(&path, DbOptions::default()).expect("create db");
    (dir, db.connect())
}

fn rows(conn: &Arc<Connection>, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let mut out = Vec::new();
    while matches!(stmt.step().expect("step"), Step::Row) {
        let cols = (0..stmt.column_count()).map(|i| {
            stmt.column_text(i)
                .map(|s| s.to_owned())
                .unwrap_or_else(|_| stmt.column_i64(i).unwrap_or(0).to_string())
        });
        out.push(cols.collect::<Vec<_>>().join("|"));
    }
    out
}

/// Case 10340: exact reproduction of the redline-testing corpus SQL.
///
/// INSERT 'apple' into a table that already holds 'Apple' under a
/// `COLLATE NOCASE` unique index. The UPSERT arm fires and updates b to
/// `upper(excluded.b)` = 'APPLE'. Result: one row with a=1, b='APPLE'.
#[test]
fn nocase_unique_index_upsert_fires_on_case_insensitive_conflict() {
    let (_dir, conn) = open();
    conn.execute("CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT)")
        .expect("create table");
    conn.execute("CREATE UNIQUE INDEX ix_t_b_ci ON t(b COLLATE NOCASE)")
        .expect("create index");
    conn.execute("INSERT INTO t VALUES(1,'Apple')")
        .expect("insert Apple");
    conn.execute(
        "INSERT INTO t VALUES(2,'apple') \
         ON CONFLICT(b COLLATE NOCASE) DO UPDATE SET b = upper(excluded.b)",
    )
    .expect("upsert apple");

    let result = rows(&conn, "SELECT a, b FROM t ORDER BY a");
    assert_eq!(
        result,
        vec!["1|APPLE"],
        "UPSERT should update row 1 to APPLE"
    );
}

/// Sanity: inserting a genuinely distinct value (case-sensitive diverge but
/// NOCASE collision) should NOT produce two rows.
#[test]
fn nocase_unique_index_blocks_duplicate() {
    let (_dir, conn) = open();
    conn.execute("CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT)")
        .expect("create table");
    conn.execute("CREATE UNIQUE INDEX ix_t_b_ci ON t(b COLLATE NOCASE)")
        .expect("create index");
    conn.execute("INSERT INTO t VALUES(1,'Apple')")
        .expect("insert Apple");
    // Attempting a plain INSERT without ON CONFLICT should fail.
    let result = conn.execute("INSERT INTO t VALUES(2,'apple')");
    assert!(
        result.is_err(),
        "plain INSERT of a NOCASE duplicate should fail with UNIQUE constraint"
    );
    // Table must still have exactly one row.
    let result = rows(&conn, "SELECT COUNT(*) FROM t");
    assert_eq!(result, vec!["1"]);
}

/// NOCASE uniqueness preserves the original casing of the stored value;
/// only the index key is normalised.
#[test]
fn nocase_unique_index_do_nothing_preserves_original_casing() {
    let (_dir, conn) = open();
    conn.execute("CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT)")
        .expect("create table");
    conn.execute("CREATE UNIQUE INDEX ix_t_b_ci ON t(b COLLATE NOCASE)")
        .expect("create index");
    conn.execute("INSERT INTO t VALUES(1,'Apple')")
        .expect("insert Apple");
    conn.execute(
        "INSERT INTO t VALUES(2,'APPLE') \
         ON CONFLICT(b COLLATE NOCASE) DO NOTHING",
    )
    .expect("upsert do-nothing");

    let result = rows(&conn, "SELECT a, b FROM t ORDER BY a");
    // Row 1 is unchanged; row 2 was silently dropped.
    assert_eq!(result, vec!["1|Apple"]);
}

#[test]
fn nocase_unique_index_backfill_rejects_existing_duplicate() {
    let (_dir, conn) = open();
    conn.execute("CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT)")
        .expect("create table");
    conn.execute("INSERT INTO t VALUES(1,'Apple')")
        .expect("insert Apple");
    conn.execute("INSERT INTO t VALUES(2,'apple')")
        .expect("insert apple");

    let result = conn.execute("CREATE UNIQUE INDEX ix_t_b_ci ON t(b COLLATE NOCASE)");
    assert!(
        result.is_err(),
        "CREATE UNIQUE INDEX should reject existing NOCASE duplicates"
    );
}

#[test]
fn nocase_unique_index_backfill_blocks_future_duplicate() {
    let (_dir, conn) = open();
    conn.execute("CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT)")
        .expect("create table");
    conn.execute("INSERT INTO t VALUES(1,'Apple')")
        .expect("insert Apple");
    conn.execute("CREATE UNIQUE INDEX ix_t_b_ci ON t(b COLLATE NOCASE)")
        .expect("create index");

    let result = conn.execute("INSERT INTO t VALUES(2,'apple')");
    assert!(
        result.is_err(),
        "backfilled NOCASE unique index should block future duplicate"
    );
}

#[test]
fn nocase_unique_index_survives_reopen_without_catalog_format_bump() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("nocase_reopen.db");
    {
        let db = Database::create(&path, DbOptions::default()).expect("create db");
        let conn = db.connect();
        conn.execute("CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT)")
            .expect("create table");
        conn.execute("CREATE UNIQUE INDEX ix_t_b_ci ON t(b COLLATE NOCASE)")
            .expect("create index");
        conn.execute("INSERT INTO t VALUES(1,'Apple')")
            .expect("insert Apple");
    }

    let db = Database::open(&path, DbOptions::default()).expect("reopen db");
    let conn = db.connect();
    conn.execute(
        "INSERT INTO t VALUES(2,'apple') \
         ON CONFLICT(b COLLATE NOCASE) DO UPDATE SET b = upper(excluded.b)",
    )
    .expect("upsert after reopen");

    let result = rows(&conn, "SELECT a, b FROM t ORDER BY a");
    assert_eq!(result, vec!["1|APPLE"]);
}

// Q5-10: a UNIQUE key on a column declared NOCASE or RTRIM compares with
// that collation, as SQLite's does, and keeps doing so after a reopen.

fn assert_unique_rejects(conn: &Arc<Connection>, table: &str, first: &str, second: &str) {
    conn.execute(&format!("INSERT INTO {table}(x) VALUES('{first}')"))
        .unwrap_or_else(|err| panic!("{table}: insert {first:?}: {err}"));
    let err = conn
        .execute(&format!("INSERT INTO {table}(x) VALUES('{second}')"))
        .expect_err(&format!(
            "{table}: {second:?} after {first:?} must violate UNIQUE"
        ));
    assert!(err.to_string().contains("UNIQUE"), "{table}: {err}");
    assert_eq!(
        rows(conn, &format!("SELECT count(*) FROM {table}")),
        vec!["1"],
        "{table}"
    );
}

const NOCASE_TABLES: [(&str, &str); 4] = [
    ("u_col", "CREATE TABLE u_col(x TEXT COLLATE NOCASE UNIQUE)"),
    (
        "u_tab",
        "CREATE TABLE u_tab(x TEXT COLLATE NOCASE, UNIQUE(x))",
    ),
    ("u_idx", "CREATE TABLE u_idx(x TEXT COLLATE NOCASE)"),
    (
        "u_pk",
        "CREATE TABLE u_pk(x TEXT COLLATE NOCASE PRIMARY KEY)",
    ),
];

#[test]
fn declared_nocase_unique_rejects_case_dup() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("declared_nocase.db");
    {
        let db = Database::create(&path, DbOptions::default()).expect("create db");
        let conn = db.connect();
        for (table, ddl) in NOCASE_TABLES {
            conn.execute(ddl)
                .unwrap_or_else(|err| panic!("{table}: {err}"));
        }
        conn.execute("CREATE UNIQUE INDEX u_idx_x ON u_idx(x)")
            .expect("unique index");
        for (table, _) in NOCASE_TABLES {
            assert_unique_rejects(&conn, table, "X", "x");
        }
    }
    // The key collation is part of the catalog, not rebuilt from SQL text.
    let db = Database::open(&path, DbOptions::default()).expect("reopen");
    let conn = db.connect();
    for (table, _) in NOCASE_TABLES {
        let err = conn
            .execute(&format!("INSERT INTO {table}(x) VALUES('x')"))
            .expect_err(&format!("{table}: 'x' after reopen must violate UNIQUE"));
        assert!(err.to_string().contains("UNIQUE"), "{table}: {err}");
        conn.execute(&format!("INSERT INTO {table}(x) VALUES('y')"))
            .unwrap_or_else(|err| panic!("{table}: a new value after reopen: {err}"));
        assert_eq!(
            rows(&conn, &format!("SELECT count(*) FROM {table} WHERE x='X'")),
            vec!["1"],
            "{table}"
        );
    }
}

#[test]
fn rtrim_unique_rejects_trailing_space() {
    let (_dir, conn) = open();
    conn.execute("CREATE TABLE r_col(x TEXT COLLATE RTRIM UNIQUE)")
        .expect("create");
    assert_unique_rejects(&conn, "r_col", "x", "x ");
    conn.execute("CREATE TABLE r_idx(x TEXT COLLATE RTRIM)")
        .expect("create");
    conn.execute("CREATE UNIQUE INDEX r_idx_x ON r_idx(x)")
        .expect("unique index");
    assert_unique_rejects(&conn, "r_idx", "x  ", "x");
    // An explicit BINARY key keeps byte comparison on an RTRIM column.
    conn.execute("CREATE TABLE r_bin(x TEXT COLLATE RTRIM)")
        .expect("create");
    conn.execute("CREATE UNIQUE INDEX r_bin_x ON r_bin(x COLLATE BINARY)")
        .expect("unique index");
    conn.execute("INSERT INTO r_bin(x) VALUES('x'), ('x ')")
        .expect("distinct under BINARY");
}

#[test]
fn an_index_key_with_a_collation_the_btree_cannot_order_is_refused() {
    let (_dir, conn) = open();
    conn.execute("CREATE TABLE t(x TEXT)").expect("create");
    let err = conn
        .execute("CREATE INDEX t_x ON t(x COLLATE \"en-x-icu\")")
        .expect_err("an ICU-collated index key");
    assert!(err.to_string().contains("index key can use only"), "{err}");
}
