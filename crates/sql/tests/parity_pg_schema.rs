//! Postgres schemas stay distinct, and a sequence default inserts values.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("schema.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

fn cell(value: &SqlValue) -> String {
    match value {
        SqlValue::Text(text) => text.to_string(),
        SqlValue::Integer(n) => n.to_string(),
        SqlValue::Null => "NULL".to_owned(),
        other => panic!("unexpected {other:?}"),
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
    conn.execute(sql)
        .unwrap_or_else(|err| panic!("{sql}: {err}"));
}

#[test]
fn postgres_schemas_and_sequence_defaults() {
    // SAFETY: this test binary has one test, so the dialect flag stays local.
    unsafe { std::env::set_var("REDLINEDB_RESULT_DIALECT", "postgres") };
    let (_dir, conn) = open();

    exec(&conn, "DROP SCHEMA IF EXISTS auth_ns CASCADE");
    exec(&conn, "DROP TABLE IF EXISTS public.users_collide");
    exec(&conn, "CREATE SCHEMA auth_ns");
    exec(
        &conn,
        "CREATE TABLE auth_ns.users_collide (id int, role text)",
    );
    exec(
        &conn,
        "CREATE TABLE public.users_collide (id int, email text)",
    );
    exec(
        &conn,
        "INSERT INTO auth_ns.users_collide VALUES (1,'admin'),(2,'user')",
    );
    exec(
        &conn,
        "INSERT INTO public.users_collide VALUES (1,'a@x'),(2,'b@x')",
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT id, role FROM auth_ns.users_collide ORDER BY id"
        ),
        "1|admin\n2|user"
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT id, email FROM public.users_collide ORDER BY id"
        ),
        "1|a@x\n2|b@x"
    );

    exec(&conn, "DROP SCHEMA IF EXISTS sch_sp CASCADE");
    exec(&conn, "CREATE SCHEMA sch_sp");
    exec(&conn, "CREATE TABLE sch_sp.widgets (id int)");
    exec(&conn, "INSERT INTO sch_sp.widgets VALUES (10),(20)");
    exec(&conn, "SET search_path TO sch_sp, public");
    assert_eq!(rows(&conn, "SELECT id FROM widgets ORDER BY id"), "10\n20");
    exec(&conn, "SET search_path TO public");

    exec(&conn, "CREATE TABLE into_src (id int)");
    exec(&conn, "INSERT INTO into_src VALUES (7)");
    assert_eq!(rows(&conn, "SELECT id FROM into_src"), "7");

    exec(&conn, "DROP SCHEMA IF EXISTS sch_auth CASCADE");
    exec(&conn, "CREATE SCHEMA sch_auth AUTHORIZATION CURRENT_USER");
    assert_eq!(
        rows(
            &conn,
            "SELECT nspname, pg_get_userbyid(nspowner) = current_user FROM pg_namespace WHERE nspname = 'sch_auth'"
        ),
        "sch_auth|t"
    );

    exec(&conn, "DROP TABLE IF EXISTS seq_owned_t");
    exec(&conn, "DROP SEQUENCE IF EXISTS seq_owned");
    exec(&conn, "CREATE SEQUENCE seq_owned");
    exec(
        &conn,
        "CREATE TABLE seq_owned_t (id int DEFAULT nextval('seq_owned'), label text)",
    );
    exec(&conn, "ALTER SEQUENCE seq_owned OWNED BY seq_owned_t.id");
    exec(&conn, "INSERT INTO seq_owned_t (label) VALUES ('a'),('b')");
    assert_eq!(
        rows(&conn, "SELECT id, label FROM seq_owned_t ORDER BY id"),
        "1|a\n2|b"
    );
}
