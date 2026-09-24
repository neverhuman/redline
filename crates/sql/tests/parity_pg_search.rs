//! Text search, trigram, and index DDL from the beyond corpus.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("search.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

fn cell(value: &SqlValue) -> String {
    match value {
        SqlValue::Text(text) => text.to_string(),
        SqlValue::Integer(n) => n.to_string(),
        SqlValue::Real(n) => {
            let rounded = (n * 1_000_000.0).round() / 1_000_000.0;
            if (rounded - rounded.round()).abs() < 1e-9 {
                format!("{}", rounded.round() as i64)
            } else {
                format!("{rounded}")
            }
        }
        SqlValue::Null => "NULL".to_owned(),
        other => format!("{other:?}"),
    }
}

fn rows(conn: &Arc<Connection>, sql: &str) -> String {
    let mut stmt = conn
        .prepare(sql)
        .unwrap_or_else(|err| panic!("{sql}: {err}"));
    let mut lines = Vec::new();
    loop {
        match stmt.step().unwrap_or_else(|err| panic!("{sql}: {err}")) {
            Step::Row => {
                let mut parts = Vec::new();
                for idx in 0..stmt.column_count() {
                    parts.push(cell(stmt.column_value(idx).expect("col")));
                }
                lines.push(parts.join("|"));
            }
            Step::Done => break,
        }
    }
    lines.join("\n")
}

fn exec(conn: &Arc<Connection>, sql: &str) {
    let mut stmt = conn
        .prepare(sql)
        .unwrap_or_else(|err| panic!("{sql}: {err}"));
    loop {
        match stmt.step().unwrap_or_else(|err| panic!("{sql}: {err}")) {
            Step::Row | Step::Done => break,
        }
    }
}

#[test]
fn search_corpus_shapes() {
    unsafe { std::env::set_var("REDLINEDB_RESULT_DIALECT", "postgres") };
    let (_dir, conn) = open();
    assert_eq!(
        rows(
            &conn,
            "SELECT to_tsvector('english', 'The quick brown fox') @@ to_tsquery('english', 'quick & fox'), to_tsvector('english', 'The quick brown fox') @@ to_tsquery('english', 'slow')"
        ),
        "t|f"
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT setweight(to_tsvector('english','quick brown fox'), 'A')::text"
        ),
        "'brown':2A 'fox':3A 'quick':1A"
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT ts_rank(to_tsvector('english','The quick brown fox'), to_tsquery('english','quick & fox')) > 0"
        ),
        "t"
    );
    exec(&conn, "CREATE EXTENSION pg_trgm");
    assert_eq!(
        rows(
            &conn,
            "SELECT similarity('apple','apply'), 'apple' % 'apply'"
        ),
        "0.5|t"
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT word_similarity('book','bookworm'), 'book' <-> 'bookworm'"
        ),
        "0.8|0.6"
    );
    exec(&conn, "CREATE TABLE trgm_t(id int, s text)");
    exec(
        &conn,
        "INSERT INTO trgm_t VALUES (1,'hello'),(2,'help'),(3,'world')",
    );
    exec(
        &conn,
        "CREATE INDEX trgm_gin ON trgm_t USING gin (s gin_trgm_ops)",
    );
    assert_eq!(
        rows(&conn, "SELECT s FROM trgm_t WHERE s % 'help' ORDER BY s"),
        "hello\nhelp"
    );
    exec(&conn, "CREATE EXTENSION btree_gin");
    exec(&conn, "CREATE TABLE gin_i(x int)");
    exec(&conn, "INSERT INTO gin_i VALUES (7),(8)");
    exec(&conn, "CREATE INDEX gin_i_x ON gin_i USING gin (x)");
    assert_eq!(rows(&conn, "SELECT count(*) FROM gin_i WHERE x = 7"), "1");
    exec(&conn, "CREATE TABLE range_t(r int4range)");
    exec(
        &conn,
        "INSERT INTO range_t VALUES (int4range(1,5)),(int4range(10,20)),(int4range(18,25))",
    );
    exec(&conn, "CREATE INDEX range_g ON range_t USING gist (r)");
    assert_eq!(
        rows(
            &conn,
            "SELECT count(*) FROM range_t WHERE r && '[19,30)'::int4range"
        ),
        "2"
    );
    let err = conn
        .prepare("CREATE EXTENSION vector")
        .err()
        .map(|err| err.to_string())
        .unwrap_or_default();
    assert!(
        err.contains("extension \"vector\" is not available"),
        "{err}"
    );
    assert_eq!(rows(&conn, "SELECT point(0,0) <-> point(3,4)"), "5");
}
