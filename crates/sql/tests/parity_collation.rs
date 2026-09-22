//! Postgres collations the beyond-SQLite corpus orders and compares with.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(dir.path().join("collation.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

fn rows(conn: &Arc<Connection>, sql: &str) -> Vec<Vec<SqlValue>> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let ncols = stmt.column_count();
    let mut out = Vec::new();
    while let Step::Row = stmt.step().expect("step") {
        out.push(
            (0..ncols)
                .map(|i| stmt.column_value(i).expect("col").clone())
                .collect(),
        );
    }
    out
}

fn text(value: &SqlValue) -> &str {
    match value {
        SqlValue::Text(text) => text.as_ref(),
        other => panic!("expected text, got {other:?}"),
    }
}

#[test]
fn collate_c_is_byte_order() {
    let (_d, c) = open();
    let got = rows(
        &c,
        "WITH v(x) AS (VALUES ('apple'),('Banana'),('Apple'),('banana')) \
         SELECT x FROM v ORDER BY x COLLATE \"C\"",
    );
    let values: Vec<&str> = got.iter().map(|row| text(&row[0])).collect();
    assert_eq!(values, ["Apple", "Banana", "apple", "banana"]);
}

#[test]
fn collate_en_x_icu_groups_case() {
    let (_d, c) = open();
    let got = rows(
        &c,
        "WITH v(x) AS (VALUES ('apple'),('Banana'),('Apple'),('banana')) \
         SELECT x FROM v ORDER BY x COLLATE \"en-x-icu\", x COLLATE \"C\"",
    );
    let values: Vec<&str> = got.iter().map(|row| text(&row[0])).collect();
    assert_eq!(values, ["apple", "Apple", "banana", "Banana"]);
}

#[test]
fn collate_icu_nulls_last() {
    let (_d, c) = open();
    let got = rows(
        &c,
        "WITH v(x) AS (VALUES ('banana'),('Apple'),(NULL),('cherry')) \
         SELECT x FROM v ORDER BY x COLLATE \"en-x-icu\" NULLS LAST",
    );
    assert!(matches!(got[3][0], SqlValue::Null), "{got:?}");
    let values: Vec<&str> = got.iter().take(3).map(|row| text(&row[0])).collect();
    assert_eq!(values, ["Apple", "banana", "cherry"]);
}

#[test]
fn custom_icu_collation_equality() {
    let (_d, c) = open();
    c.execute(
        "CREATE COLLATION beyond_ci (provider = icu, locale = 'und-u-ks-level2', deterministic = false)",
    )
    .expect("ci");
    c.execute(
        "CREATE COLLATION beyond_ai (provider = icu, locale = 'und-u-ks-level1', deterministic = false)",
    )
    .expect("ai");
    let ci = rows(
        &c,
        "SELECT 'ABC' = 'abc' COLLATE beyond_ci, 'ABC' = 'xyz' COLLATE beyond_ci",
    );
    assert_eq!(ci[0][0], SqlValue::Integer(1));
    assert_eq!(ci[0][1], SqlValue::Integer(0));
    let ai = rows(
        &c,
        "SELECT 'cafe' = 'café' COLLATE beyond_ai, 'Cafe' = 'CAFÉ' COLLATE beyond_ai",
    );
    assert_eq!(ai[0][0], SqlValue::Integer(1));
    assert_eq!(ai[0][1], SqlValue::Integer(1));
}
