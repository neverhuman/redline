//! ALTER TABLE inheritance and catalog flags used by the Postgres corpus.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("alter.db"), DbOptions::default()).expect("db");
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
fn postgres_alter_inherit_and_catalog_flags() {
    unsafe { std::env::set_var("REDLINEDB_RESULT_DIALECT", "postgres") };
    let (_dir, conn) = open();
    exec(&conn, "CREATE TABLE mig_inh_parent (id int, label text)");
    exec(&conn, "CREATE TABLE mig_inh_child (id int, label text)");
    exec(&conn, "INSERT INTO mig_inh_parent VALUES (1,'p')");
    exec(&conn, "INSERT INTO mig_inh_child VALUES (2,'c')");
    assert_eq!(
        rows(
            &conn,
            "SELECT id, label FROM (SELECT * FROM mig_inh_parent UNION ALL SELECT * FROM mig_inh_child) AS __inh ORDER BY id"
        ),
        "1|p\n2|c"
    );
    exec(&conn, "ALTER TABLE mig_inh_child INHERIT mig_inh_parent");
    assert_eq!(
        rows(&conn, "SELECT id, label FROM mig_inh_parent ORDER BY id"),
        "1|p\n2|c"
    );
    exec(&conn, "ALTER TABLE mig_inh_child NO INHERIT mig_inh_parent");
    assert_eq!(
        rows(&conn, "SELECT id, label FROM mig_inh_parent ORDER BY id"),
        "1|p"
    );

    exec(&conn, "CREATE TABLE mig_logged (id int)");
    exec(&conn, "ALTER TABLE mig_logged SET UNLOGGED");
    assert_eq!(
        rows(
            &conn,
            "SELECT relpersistence FROM pg_class WHERE relname = 'mig_logged'"
        ),
        "u"
    );
    exec(&conn, "ALTER TABLE mig_logged SET LOGGED");
    assert_eq!(
        rows(
            &conn,
            "SELECT relpersistence FROM pg_class WHERE relname = 'mig_logged'"
        ),
        "p"
    );

    exec(&conn, "CREATE TABLE mig_stats (id int, label text)");
    exec(
        &conn,
        "ALTER TABLE mig_stats ALTER COLUMN label SET STATISTICS 250",
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT attstattarget FROM pg_attribute WHERE attrelid = 'mig_stats' AND attname = 'label'"
        ),
        "250"
    );

    exec(&conn, "CREATE TABLE mig_storage (id int, body text)");
    exec(
        &conn,
        "ALTER TABLE mig_storage ALTER COLUMN body SET STORAGE EXTERNAL",
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT attstorage FROM pg_attribute WHERE attrelid = 'mig_storage' AND attname = 'body'"
        ),
        "e"
    );

    exec(&conn, "CREATE TABLE mig_reloptions (id int)");
    exec(
        &conn,
        "ALTER TABLE mig_reloptions SET (autovacuum_enabled = true)",
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT array_to_string(reloptions, ',') FROM pg_class WHERE relname = 'mig_reloptions'"
        ),
        "autovacuum_enabled=true"
    );

    exec(&conn, "CREATE TABLE mig_owner_cluster (id int)");
    exec(&conn, "CREATE INDEX mig_oc_idx ON mig_owner_cluster(id)");
    exec(&conn, "ALTER TABLE mig_owner_cluster CLUSTER ON mig_oc_idx");
    exec(&conn, "ALTER TABLE mig_owner_cluster OWNER TO CURRENT_USER");
    assert_eq!(
        rows(
            &conn,
            "SELECT tableowner FROM pg_tables WHERE tablename = 'mig_owner_cluster'"
        ),
        "redlinedb"
    );
    exec(&conn, "ALTER TABLE mig_owner_cluster SET WITHOUT CLUSTER");
    assert_eq!(
        rows(
            &conn,
            "SELECT count(*) FROM pg_index WHERE indrelid = 'mig_owner_cluster' AND indisclustered"
        ),
        "0"
    );
}
