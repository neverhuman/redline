//! Materialized views keep a snapshot until REFRESH.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("mv.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

fn column_text(value: &SqlValue) -> String {
    match value {
        SqlValue::Text(text) => text.to_string(),
        SqlValue::Integer(n) => n.to_string(),
        SqlValue::Real(n) => {
            if n.fract() == 0.0 {
                format!("{}", *n as i64)
            } else {
                n.to_string()
            }
        }
        other => panic!("unexpected value {other:?}"),
    }
}

fn rows(conn: &Arc<Connection>, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).expect(sql);
    let mut out = Vec::new();
    loop {
        match stmt.step().expect(sql) {
            Step::Row => {
                let mut parts = Vec::new();
                for idx in 0..stmt.column_count() {
                    parts.push(column_text(stmt.column_value(idx).expect("value")));
                }
                out.push(parts.join("|"));
            }
            Step::Done => break,
        }
    }
    out
}

#[test]
fn materialized_views_snapshot_and_refresh() {
    // SAFETY: this test binary has one test, so the dialect flag stays local.
    unsafe { std::env::set_var("REDLINEDB_RESULT_DIALECT", "postgres") };
    let (_dir, conn) = open();

    conn.execute("DROP MATERIALIZED VIEW IF EXISTS mv_missing")
        .expect("drop missing");
    conn.execute("CREATE TABLE src (id int, label text)")
        .expect("table");
    conn.execute("INSERT INTO src VALUES (1,'a'),(2,'b')")
        .expect("insert");
    conn.execute("CREATE MATERIALIZED VIEW mv AS SELECT id, label FROM src")
        .expect("create");
    assert_eq!(
        rows(&conn, "SELECT id, label FROM mv ORDER BY id"),
        vec!["1|a".to_owned(), "2|b".to_owned()]
    );
    conn.execute("INSERT INTO src VALUES (3,'c')")
        .expect("more");
    assert_eq!(
        rows(&conn, "SELECT id FROM mv ORDER BY id"),
        vec!["1".to_owned(), "2".to_owned()]
    );
    conn.execute("REFRESH MATERIALIZED VIEW mv")
        .expect("refresh");
    assert_eq!(
        rows(&conn, "SELECT id FROM mv ORDER BY id"),
        vec!["1".to_owned(), "2".to_owned(), "3".to_owned()]
    );

    conn.execute("CREATE MATERIALIZED VIEW mv_empty AS SELECT id FROM src WITH NO DATA")
        .expect("no data");
    assert_eq!(
        rows(
            &conn,
            "SELECT relispopulated FROM pg_class WHERE relname = 'mv_empty'"
        ),
        vec!["f".to_owned()]
    );
    conn.execute("REFRESH MATERIALIZED VIEW mv_empty")
        .expect("populate");
    assert_eq!(
        rows(
            &conn,
            "SELECT relispopulated FROM pg_class WHERE relname = 'mv_empty'"
        ),
        vec!["t".to_owned()]
    );

    conn.execute("CREATE UNIQUE INDEX mv_uq ON mv(id)")
        .expect("index");
    assert_eq!(
        rows(
            &conn,
            "SELECT indexname FROM pg_indexes WHERE tablename = 'mv' ORDER BY indexname"
        ),
        vec!["mv_uq".to_owned()]
    );
    conn.execute("INSERT INTO src VALUES (4,'d')")
        .expect("four");
    conn.execute("REFRESH MATERIALIZED VIEW CONCURRENTLY mv")
        .expect("concurrent");
    assert_eq!(rows(&conn, "SELECT count(*) FROM mv"), vec!["4".to_owned()]);

    conn.execute("CREATE MATERIALIZED VIEW mv_dep AS SELECT id FROM mv")
        .expect("dep");
    conn.execute("DROP MATERIALIZED VIEW mv CASCADE")
        .expect("cascade");
    assert_eq!(
        rows(
            &conn,
            "SELECT count(*) FROM pg_matviews WHERE matviewname IN ('mv','mv_dep')"
        ),
        vec!["0".to_owned()]
    );

    conn.execute("CREATE MATERIALIZED VIEW mv_old AS SELECT id FROM src")
        .expect("old");
    conn.execute("ALTER MATERIALIZED VIEW mv_old RENAME TO mv_new")
        .expect("rename");
    assert_eq!(
        rows(
            &conn,
            "SELECT matviewname FROM pg_matviews WHERE matviewname IN ('mv_old','mv_new')"
        ),
        vec!["mv_new".to_owned()]
    );
    assert_eq!(
        rows(&conn, "SELECT count(*) FROM mv_new"),
        vec!["4".to_owned()]
    );
}
