//! PG-08: behaviour that only a second session can observe.
//!
//! Every beyond-SQLite corpus case runs in one fresh process on `:memory:`,
//! so a stand-in that takes no lock, delivers nothing or returns a constant
//! passes it. These tests open one Postgres-dialect database and two
//! connections to it, and check what one connection's actions do to the
//! other. They exercise the native API, not the PostgreSQL wire protocol.

use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use redlinedb_sql::{Connection, Database, DbOptions, Dialect, SqlValue, Step};

#[path = "pg_behavior_two_sessions/advisory_locks.rs"]
mod advisory_locks;

/// Long enough that a lock wait never times out by accident, short enough
/// that a test which does wait for it stays quick.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

fn open_pg() -> (tempfile::TempDir, Arc<Database>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let options = DbOptions {
        dialect: Some(Dialect::PostgresSubset),
        busy_timeout: BUSY_TIMEOUT,
        ..DbOptions::default()
    };
    let db = Database::create(dir.path().join("two.db"), options).expect("create db");
    (dir, db)
}

fn rows(conn: &Arc<Connection>, sql: &str) -> Vec<Vec<SqlValue>> {
    let mut stmt = conn
        .prepare(sql)
        .unwrap_or_else(|err| panic!("{sql}: {err}"));
    let mut out = Vec::new();
    while let Step::Row = stmt.step().unwrap_or_else(|err| panic!("{sql}: {err}")) {
        out.push(
            (0..stmt.column_count())
                .map(|i| stmt.column_value(i).expect("column").clone())
                .collect(),
        );
    }
    out
}

fn one(conn: &Arc<Connection>, sql: &str) -> SqlValue {
    let rows = rows(conn, sql);
    assert_eq!(rows.len(), 1, "{sql}: {rows:?}");
    assert_eq!(rows[0].len(), 1, "{sql}: {rows:?}");
    rows[0][0].clone()
}

fn error_of(conn: &Arc<Connection>, sql: &str) -> String {
    let mut stmt = match conn.prepare(sql) {
        Ok(stmt) => stmt,
        Err(err) => return err.to_string(),
    };
    loop {
        match stmt.step() {
            Ok(Step::Row) => continue,
            Ok(Step::Done) => panic!("{sql} succeeded"),
            Err(err) => return err.to_string(),
        }
    }
}

fn text(value: &str) -> SqlValue {
    SqlValue::Text(Arc::from(value))
}

fn t() -> SqlValue {
    text("t")
}

fn f() -> SqlValue {
    text("f")
}

/// `void` prints as an empty cell in psql.
fn void() -> SqlValue {
    text("")
}

fn integer(value: SqlValue) -> i64 {
    match value {
        SqlValue::Integer(n) => n,
        other => panic!("expected an integer, got {other:?}"),
    }
}

#[test]
fn listen_channels_belong_to_one_session() {
    let (_dir, db) = open_pg();
    let a = db.connect();
    let b = db.connect();
    a.execute("LISTEN chan_a").expect("listen a");
    b.execute("LISTEN chan_b").expect("listen b");
    let channels = |conn: &Arc<Connection>| rows(conn, "SELECT pg_listening_channels()");
    assert_eq!(channels(&a), vec![vec![text("chan_a")]]);
    assert_eq!(channels(&b), vec![vec![text("chan_b")]]);
    b.execute("UNLISTEN *").expect("unlisten b");
    assert_eq!(channels(&a), vec![vec![text("chan_a")]]);
    assert!(channels(&b).is_empty());
    assert_eq!(
        one(
            &b,
            "SELECT count(*) FROM (SELECT pg_listening_channels()) s"
        ),
        SqlValue::Integer(0)
    );
}

#[test]
fn notify_is_refused_even_with_a_listener() {
    let (_dir, db) = open_pg();
    let a = db.connect();
    let b = db.connect();
    b.execute("LISTEN chan").expect("listen");
    for sql in ["NOTIFY chan", "NOTIFY chan, 'payload'"] {
        let err = a.prepare(sql).err().map(|err| err.to_string());
        assert!(
            err.as_deref()
                .is_some_and(|err| err.contains("unsupported capability: NOTIFY")),
            "{sql}: {err:?}"
        );
    }
    let err = error_of(&a, "SELECT pg_notify('chan', 'payload')");
    assert!(err.contains("unsupported capability: pg_notify"), "{err}");
    // Nothing was queued for the listener.
    assert_eq!(
        one(&b, "SELECT pg_notification_queue_usage()"),
        SqlValue::Real(0.0)
    );
}

#[test]
fn concurrent_transactions_have_distinct_transaction_ids() {
    let (_dir, db) = open_pg();
    let a = db.connect();
    let b = db.connect();
    a.execute("CREATE TABLE tx_t(id int)").expect("create");
    a.execute("BEGIN").expect("begin a");
    b.execute("BEGIN").expect("begin b");
    let a_first = integer(one(&a, "SELECT txid_current()"));
    let b_first = integer(one(&b, "SELECT txid_current()"));
    assert!(a_first > 0 && b_first > 0, "{a_first} {b_first}");
    assert_ne!(a_first, b_first, "two open transactions share an id");
    // Stable for the life of a transaction, across reads and writes.
    a.execute("INSERT INTO tx_t VALUES (1)").expect("insert");
    assert_eq!(integer(one(&a, "SELECT txid_current()")), a_first);
    assert_eq!(integer(one(&a, "SELECT txid_current() FROM tx_t")), a_first);
    assert_eq!(integer(one(&a, "SELECT pg_current_xact_id()")), a_first);
    assert_eq!(integer(one(&b, "SELECT txid_current()")), b_first);
    a.execute("COMMIT").expect("commit a");
    b.execute("COMMIT").expect("commit b");
    // The next transaction gets a new id.
    a.execute("BEGIN").expect("begin a again");
    let a_second = integer(one(&a, "SELECT txid_current()"));
    assert!(
        a_second != a_first && a_second != b_first,
        "{a_second} reused an id"
    );
    a.execute("COMMIT").expect("commit a again");
    // Each autocommit statement is its own transaction, as in PostgreSQL.
    let first = integer(one(&a, "SELECT txid_current()"));
    let second = integer(one(&a, "SELECT txid_current()"));
    assert_ne!(first, second);
}

#[test]
fn a_table_created_in_one_session_is_in_the_catalogs_of_the_other() {
    let (_dir, db) = open_pg();
    let a = db.connect();
    let b = db.connect();
    let relations = "SELECT relname, relkind FROM pg_class \
                     WHERE relname LIKE 'shared_%' ORDER BY relname";
    let schema = "SELECT count(*) FROM sqlite_master WHERE name LIKE 'shared_%'";
    assert!(rows(&b, relations).is_empty());
    a.execute("CREATE TABLE shared_t(id int, name text)")
        .expect("create table");
    a.execute("CREATE INDEX shared_i ON shared_t(id)")
        .expect("create index");
    a.execute("CREATE VIEW shared_v AS SELECT id FROM shared_t")
        .expect("create view");
    let expected = vec![
        vec![text("shared_i"), text("i")],
        vec![text("shared_t"), text("r")],
        vec![text("shared_v"), text("v")],
    ];
    assert_eq!(rows(&b, relations), expected);
    assert_eq!(rows(&a, relations), expected);
    assert_eq!(one(&b, schema), SqlValue::Integer(3));
    assert_eq!(
        one(&b, "SELECT count(*) FROM shared_v"),
        SqlValue::Integer(0)
    );
    a.execute("DROP VIEW shared_v").expect("drop view");
    a.execute("DROP TABLE shared_t").expect("drop table");
    assert!(rows(&b, relations).is_empty());
    assert_eq!(one(&b, schema), SqlValue::Integer(0));
}

#[test]
fn a_refreshed_materialized_view_is_refreshed_for_the_other_session() {
    let (_dir, db) = open_pg();
    let a = db.connect();
    let b = db.connect();
    a.execute("CREATE TABLE mv_base(id int)").expect("create");
    a.execute("INSERT INTO mv_base VALUES (1), (2)")
        .expect("insert");
    a.execute("CREATE MATERIALIZED VIEW mv_count AS SELECT count(*) AS n FROM mv_base")
        .expect("create matview");
    assert_eq!(one(&b, "SELECT n FROM mv_count"), SqlValue::Integer(2));
    // A base-table change is not visible until the view is refreshed ...
    b.execute("INSERT INTO mv_base VALUES (3)").expect("insert");
    assert_eq!(one(&a, "SELECT n FROM mv_count"), SqlValue::Integer(2));
    assert_eq!(one(&b, "SELECT n FROM mv_count"), SqlValue::Integer(2));
    // ... and a refresh in one session is what the other one reads.
    a.execute("REFRESH MATERIALIZED VIEW mv_count")
        .expect("refresh");
    assert_eq!(one(&b, "SELECT n FROM mv_count"), SqlValue::Integer(3));
}

/// Known gap, recorded in the `multi_session` row of
/// `metadata/beyond_sqlite/postgres-capabilities.json`: sequences and the
/// `pg_matviews` catalog are kept per connection, so a second session does
/// not see them. This test pins the current behaviour; the day it fails,
/// update the capability note with it.
#[test]
fn sequences_and_the_matview_catalog_are_still_per_session() {
    let (_dir, db) = open_pg();
    let a = db.connect();
    let b = db.connect();
    a.execute("CREATE SEQUENCE shared_seq")
        .expect("create sequence");
    assert_eq!(
        one(&a, "SELECT nextval('shared_seq')"),
        SqlValue::Integer(1)
    );
    let err = error_of(&b, "SELECT nextval('shared_seq')");
    assert!(err.contains("does not exist"), "{err}");
    a.execute("CREATE TABLE mv_src(id int)").expect("create");
    a.execute("CREATE MATERIALIZED VIEW mv_seen AS SELECT id FROM mv_src")
        .expect("create matview");
    let listed = "SELECT count(*) FROM pg_matviews WHERE matviewname = 'mv_seen'";
    assert_eq!(one(&a, listed), SqlValue::Integer(1));
    assert_eq!(one(&b, listed), SqlValue::Integer(0));
}
