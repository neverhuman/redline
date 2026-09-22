//! `LANGUAGE SQL` functions substitute arguments and run one SELECT.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("fn.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

fn cell(value: &SqlValue) -> String {
    match value {
        SqlValue::Text(text) => text.to_string(),
        SqlValue::Integer(n) => n.to_string(),
        other => panic!("unexpected {other:?}"),
    }
}

fn row(conn: &Arc<Connection>, sql: &str) -> String {
    let mut stmt = conn.prepare(sql).expect(sql);
    let Step::Row = stmt.step().expect(sql) else {
        panic!("no row for {sql}");
    };
    let mut parts = Vec::new();
    for idx in 0..stmt.column_count() {
        parts.push(cell(stmt.column_value(idx).expect("col")));
    }
    parts.join("|")
}

#[test]
fn language_sql_functions_substitute_arguments() {
    // SAFETY: this test binary has one test, so the dialect flag stays local.
    unsafe { std::env::set_var("REDLINEDB_RESULT_DIALECT", "postgres") };
    let (_dir, conn) = open();
    conn.execute("DROP FUNCTION IF EXISTS bsp_missing(int)")
        .expect("drop missing");
    conn.execute(
        "CREATE FUNCTION bsp_add_sql(a int, b int) RETURNS int LANGUAGE SQL AS $$ SELECT a + b; $$",
    )
    .expect("add");
    assert_eq!(row(&conn, "SELECT bsp_add_sql(2, 40)"), "42");

    conn.execute(
        "CREATE FUNCTION bsp_greet(who text DEFAULT 'world') RETURNS text LANGUAGE SQL AS $$ SELECT 'hi ' || who; $$",
    )
    .expect("greet");
    assert_eq!(
        row(&conn, "SELECT bsp_greet(), bsp_greet('ada')"),
        "hi world|hi ada"
    );

    conn.execute(
        "CREATE FUNCTION bsp_imm(n int) RETURNS int LANGUAGE SQL IMMUTABLE AS $$ SELECT n + 1; $$",
    )
    .expect("imm");
    conn.execute(
        "CREATE FUNCTION bsp_sta(n int) RETURNS int LANGUAGE SQL STABLE AS $$ SELECT n + 2; $$",
    )
    .expect("sta");
    conn.execute(
        "CREATE FUNCTION bsp_vol(n int) RETURNS int LANGUAGE SQL VOLATILE AS $$ SELECT n + 3; $$",
    )
    .expect("vol");
    assert_eq!(
        row(&conn, "SELECT bsp_imm(10), bsp_sta(10), bsp_vol(10)"),
        "11|12|13"
    );

    conn.execute(
        "CREATE FUNCTION bsp_sdef(n int) RETURNS int LANGUAGE SQL SECURITY DEFINER AS $$ SELECT n; $$",
    )
    .expect("sdef");
    conn.execute(
        "CREATE FUNCTION bsp_sinv(n int) RETURNS int LANGUAGE SQL SECURITY INVOKER AS $$ SELECT n; $$",
    )
    .expect("sinv");
    assert_eq!(
        row(
            &conn,
            "SELECT proname, prosecdef FROM pg_proc WHERE proname IN ('bsp_sdef','bsp_sinv') ORDER BY proname"
        ),
        "bsp_sdef|t"
    );
    // The first row is sdef. Read the second by stepping again.
    let mut stmt = conn
        .prepare(
            "SELECT proname, prosecdef FROM pg_proc WHERE proname IN ('bsp_sdef','bsp_sinv') ORDER BY proname",
        )
        .expect("proc");
    assert!(matches!(stmt.step().expect("r1"), Step::Row));
    assert_eq!(
        format!(
            "{}|{}",
            cell(stmt.column_value(0).unwrap()),
            cell(stmt.column_value(1).unwrap())
        ),
        "bsp_sdef|t"
    );
    assert!(matches!(stmt.step().expect("r2"), Step::Row));
    assert_eq!(
        format!(
            "{}|{}",
            cell(stmt.column_value(0).unwrap()),
            cell(stmt.column_value(1).unwrap())
        ),
        "bsp_sinv|f"
    );
}
