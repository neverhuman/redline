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
    // NOTIFY is refused (PG-01); the refusal does not end the transaction.
    let err = conn
        .execute("NOTIFY beyond_ch_tx, 'discarded'")
        .expect_err("notify");
    assert!(
        err.to_string().contains("unsupported capability: NOTIFY"),
        "{err}"
    );
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

// PG-02: the `pg_listening_channels()` rewrite searched the raw text, so the
// call spelled inside a literal was rewritten (and, after LISTEN, the channel
// list leaked into the literal), and a prepared statement kept the channel
// set of the moment it was prepared. The tests below open a Postgres-dialect
// database explicitly, so they do not depend on the environment variable the
// test above sets.

fn open_postgres() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let options = DbOptions {
        dialect: Some(redlinedb_sql::Dialect::PostgresSubset),
        ..DbOptions::default()
    };
    let db = Database::create(dir.path().join("listen_pg.db"), options).expect("db");
    (dir, db.connect())
}

fn row(conn: &Arc<Connection>, sql: &str) -> (Vec<String>, Vec<SqlValue>) {
    let mut stmt = conn
        .prepare(sql)
        .unwrap_or_else(|err| panic!("prepare `{sql}`: {err}"));
    let names = (0..stmt.column_count())
        .map(|index| stmt.column_name(index).to_owned())
        .collect();
    assert!(
        matches!(stmt.step().expect("step"), Step::Row),
        "`{sql}` returned no row"
    );
    let values = (0..stmt.column_count())
        .map(|index| stmt.column_value(index).expect("value").clone())
        .collect();
    (names, values)
}

fn text(value: &str) -> SqlValue {
    SqlValue::Text(Arc::from(value))
}

#[test]
fn listening_channels_text_in_literals_is_returned_byte_for_byte() {
    let (_dir, conn) = open_postgres();
    conn.execute("LISTEN leak_a").expect("listen");
    let call = "SELECT pg_listening_channels()";
    for (sql, want) in [
        (format!("SELECT '{call}'"), call.to_owned()),
        (format!("SELECT 'it''s {call}'"), format!("it's {call}")),
        (format!("SELECT E'it\\'s {call}'"), format!("it's {call}")),
        (format!("SELECT $${call}$$"), call.to_owned()),
        (format!("SELECT $t${call}$t$"), call.to_owned()),
        (format!("SELECT 'é {call} ü'"), format!("é {call} ü")),
    ] {
        let (_, values) = row(&conn, &sql);
        assert_eq!(values, vec![text(&want)], "`{sql}`");
    }
    let quoted = format!("SELECT 7 AS \"{call}\"");
    let (names, values) = row(&conn, &quoted);
    assert_eq!(names, vec![call.to_owned()], "`{quoted}`");
    assert_eq!(values, vec![SqlValue::Integer(7)], "`{quoted}`");
    for sql in [
        format!("SELECT 7 -- {call}"),
        format!("SELECT 7 /* {call} /* nested */ */"),
    ] {
        let (_, values) = row(&conn, &sql);
        assert_eq!(values, vec![SqlValue::Integer(7)], "`{sql}`");
    }
}

#[test]
fn listening_channels_call_spellings() {
    let (_dir, conn) = open_postgres();
    conn.execute("LISTEN spell_b").expect("listen b");
    conn.execute("LISTEN spell_a").expect("listen a");
    let want = vec!["spell_a".to_owned(), "spell_b".to_owned()];
    for sql in [
        "Select PG_LISTENING_CHANNELS() ORDER BY 1",
        "SELECT pg_listening_channels ( ) ORDER BY 1",
        "/* café */ SELECT pg_listening_channels() ORDER BY 1",
        "SELECT pg_listening_channels() /* ü */ ORDER BY 1;",
    ] {
        assert_eq!(texts(&conn, sql), want, "`{sql}`");
    }
    assert_eq!(
        texts(
            &conn,
            "SELECT count(*) FROM (SELECT pg_listening_channels()) s"
        ),
        vec!["2".to_owned()]
    );
}

#[test]
fn listening_channels_outside_a_bare_select_list_fail_closed() {
    let (_dir, conn) = open_postgres();
    conn.execute("LISTEN closed_a").expect("listen");
    for sql in [
        "SELECT pg_listening_channels(), 1",
        "SELECT 1, pg_listening_channels()",
        "SELECT upper(pg_listening_channels())",
        "SELECT * FROM pg_listening_channels()",
        "SELECT pg_listening_channels() AS channel",
    ] {
        let err = match conn.prepare(sql) {
            Ok(_) => panic!("`{sql}` was accepted"),
            Err(err) => err,
        };
        assert!(
            err.to_string()
                .contains("pg_listening_channels outside a bare SELECT list"),
            "`{sql}`: {err}"
        );
    }
}

#[test]
fn prepared_listening_channels_reflect_the_current_set() {
    let (_dir, conn) = open_postgres();
    let mut stmt = conn
        .prepare("SELECT pg_listening_channels() ORDER BY 1")
        .expect("prepare once");
    let run = |stmt: &mut redlinedb_sql::Statement| {
        stmt.reset().expect("reset");
        let mut out = Vec::new();
        while let Step::Row = stmt.step().expect("step") {
            out.push(column_text(stmt.column_value(0).expect("value")));
        }
        out
    };
    assert_eq!(run(&mut stmt), Vec::<String>::new(), "before LISTEN");
    conn.execute("LISTEN prep_a").expect("listen a");
    assert_eq!(run(&mut stmt), vec!["prep_a".to_owned()], "after LISTEN a");
    conn.execute("LISTEN prep_b").expect("listen b");
    assert_eq!(
        run(&mut stmt),
        vec!["prep_a".to_owned(), "prep_b".to_owned()],
        "after LISTEN b"
    );
    conn.execute("UNLISTEN *").expect("unlisten");
    assert_eq!(run(&mut stmt), Vec::<String>::new(), "after UNLISTEN *");
}
