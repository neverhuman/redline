//! `LISTEN` / `UNLISTEN` keep a session channel set. Rollback drops listens
//! that have not committed. `pg_listening_channels()` reads the set.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("listen.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

fn column_text(value: &SqlValue) -> String {
    match value {
        SqlValue::Text(text) => text.to_string(),
        SqlValue::Integer(n) => n.to_string(),
        other => panic!("unexpected value {other:?}"),
    }
}

fn texts(conn: &Arc<Connection>, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let mut out = Vec::new();
    loop {
        match stmt.step().expect("step") {
            Step::Row => out.push(column_text(stmt.column_value(0).expect("value"))),
            Step::Done => break,
        }
    }
    out
}

#[test]
fn listen_tracks_channels_and_rolls_back() {
    // SAFETY: this test binary has one test, so the dialect flag stays local.
    unsafe { std::env::set_var("REDLINEDB_RESULT_DIALECT", "postgres") };
    let (_dir, conn) = open();

    conn.execute("LISTEN beyond_ch_a").expect("listen a");
    assert_eq!(
        texts(&conn, "SELECT pg_listening_channels()"),
        vec!["beyond_ch_a".to_owned()]
    );
    conn.execute("UNLISTEN beyond_ch_a").expect("unlisten a");
    assert_eq!(
        texts(&conn, "SELECT pg_listening_channels()"),
        Vec::<String>::new(),
        "after unlisten"
    );

    conn.execute("LISTEN beyond_ch_x").expect("listen x");
    conn.execute("LISTEN beyond_ch_y").expect("listen y");
    conn.execute("UNLISTEN beyond_ch_x").expect("unlisten x");
    assert_eq!(
        texts(&conn, "SELECT pg_listening_channels()"),
        vec!["beyond_ch_y".to_owned()]
    );
    conn.execute("UNLISTEN *").expect("unlisten star");
    assert_eq!(
        texts(
            &conn,
            "SELECT count(*) FROM (SELECT pg_listening_channels()) s"
        ),
        vec!["0".to_owned()]
    );

    conn.execute("BEGIN").expect("begin");
    conn.execute("LISTEN beyond_ch_tx").expect("listen tx");
    conn.execute("NOTIFY beyond_ch_tx, 'discarded'")
        .expect("notify");
    conn.execute("ROLLBACK").expect("rollback");
    assert_eq!(
        texts(
            &conn,
            "SELECT count(*) FROM (SELECT pg_listening_channels()) s"
        ),
        vec!["0".to_owned()]
    );

    conn.execute("LISTEN beyond_ch_m1").expect("m1");
    conn.execute("LISTEN beyond_ch_m2").expect("m2");
    conn.execute("LISTEN beyond_ch_m3").expect("m3");
    assert_eq!(
        texts(&conn, "SELECT pg_listening_channels() ORDER BY 1"),
        vec![
            "beyond_ch_m1".to_owned(),
            "beyond_ch_m2".to_owned(),
            "beyond_ch_m3".to_owned()
        ]
    );

    let err = conn.execute("LISTEN ALL").expect_err("listen all");
    assert!(err.to_string().contains("ALL"), "{err}");

    conn.execute("BEGIN").expect("begin keep");
    conn.execute("LISTEN beyond_keep").expect("listen keep");
    conn.execute("COMMIT").expect("commit keep");
    conn.execute("BEGIN").expect("begin undo");
    conn.execute("UNLISTEN beyond_keep").expect("unlisten keep");
    conn.execute("ROLLBACK").expect("rollback undo");
    let channels = texts(&conn, "SELECT pg_listening_channels() ORDER BY 1");
    assert!(
        channels.iter().any(|ch| ch == "beyond_keep"),
        "{channels:?}"
    );
}
