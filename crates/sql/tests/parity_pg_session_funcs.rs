//! The Postgres session functions do what their names say, or refuse.
//!
//! PG-01: these used to be single-connection stand-ins: a constant
//! transaction id and WAL LSN, advisory-lock predicates that took no lock and
//! answered `t` for a key nobody held, and a `pg_wal_lsn_diff` that returned
//! 1 for any two different LSNs. Two-session behaviour is in
//! `pg_behavior_two_sessions.rs`.

use redlinedb_sql::{Connection, Database, DbOptions, Dialect, SqlValue, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let options = DbOptions {
        dialect: Some(Dialect::PostgresSubset),
        ..DbOptions::default()
    };
    let db = Database::create(dir.path().join("sessfn.db"), options).expect("db");
    (dir, db.connect())
}

fn row(conn: &Arc<Connection>, sql: &str) -> Vec<SqlValue> {
    let mut stmt = conn.prepare(sql).expect(sql);
    assert!(matches!(stmt.step().expect(sql), Step::Row), "{sql}");
    (0..stmt.column_count())
        .map(|index| stmt.column_value(index).expect(sql).clone())
        .collect()
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

#[test]
fn session_functions_match_the_shell_shapes() {
    let (_d, c) = open();
    let t = text("t");
    let f = text("f");
    // Partial: one OS process id shared by every connection.
    assert_eq!(
        row(
            &c,
            "SELECT pg_backend_pid() = pg_backend_pid(), pg_backend_pid() > 0"
        ),
        vec![t.clone(), t.clone()]
    );

    // Transaction ids come from the open transaction.
    c.execute("BEGIN").expect("begin");
    assert_eq!(row(&c, "SELECT txid_current() > 0"), vec![t.clone()]);
    assert_eq!(
        row(&c, "SELECT pg_current_xact_id()::text ~ '^[0-9]+$'"),
        vec![t.clone()]
    );
    let first = row(&c, "SELECT txid_current()");
    assert_eq!(row(&c, "SELECT txid_current()"), first);
    assert_eq!(row(&c, "SELECT pg_current_xact_id()"), first);
    assert!(
        matches!(first[..], [SqlValue::Integer(id)] if id > 0),
        "{first:?}"
    );
    let err = error_of(&c, "SELECT pg_export_snapshot()");
    assert!(err.contains("unsupported capability"), "{err}");
    c.execute("ROLLBACK").expect("rollback");
    c.execute("BEGIN").expect("begin again");
    // The stand-in answered 1 in every transaction.
    assert_ne!(row(&c, "SELECT txid_current()"), first);
    c.execute("COMMIT").expect("commit");

    // Advisory locks: `void` results print empty, unlock answers whether
    // this session held the key, and locks nest.
    assert_eq!(
        row(&c, "SELECT pg_advisory_lock(987001) IS NULL"),
        vec![f.clone()]
    );
    assert_eq!(row(&c, "SELECT pg_advisory_lock(987001)"), vec![text("")]);
    assert_eq!(
        row(&c, "SELECT pg_advisory_unlock(987001)"),
        vec![t.clone()]
    );
    assert_eq!(
        row(&c, "SELECT pg_advisory_unlock(987001)"),
        vec![t.clone()]
    );
    assert_eq!(
        row(&c, "SELECT pg_advisory_unlock(987001)"),
        vec![f.clone()]
    );
    assert_eq!(
        row(&c, "SELECT pg_try_advisory_lock(987002)"),
        vec![t.clone()]
    );
    assert_eq!(
        row(&c, "SELECT pg_advisory_unlock(987002)"),
        vec![t.clone()]
    );
    assert_eq!(
        row(&c, "SELECT pg_advisory_unlock(912345)"),
        vec![f.clone()]
    );
    assert_eq!(row(&c, "SELECT pg_advisory_unlock_all()"), vec![text("")]);

    // pg_wal_lsn_diff is the signed byte distance between two LSNs.
    assert_eq!(
        row(
            &c,
            "SELECT pg_wal_lsn_diff('0/10', '0/0'), pg_wal_lsn_diff('0/0', '0/10')"
        ),
        vec![SqlValue::Integer(16), SqlValue::Integer(-16)]
    );
    assert_eq!(
        row(
            &c,
            "SELECT pg_wal_lsn_diff('16/B374D848', '16/B374D848'), \
             pg_wal_lsn_diff('1/0', '0/FFFFFFFF'), pg_wal_lsn_diff(NULL, '0/0')"
        ),
        vec![SqlValue::Integer(0), SqlValue::Integer(1), SqlValue::Null]
    );
    for bad in [
        "'bogus'",
        "'0/'",
        "'/1'",
        "'1/2/3'",
        "'123456789/0'",
        "' 0/1'",
    ] {
        let err = error_of(&c, &format!("SELECT pg_wal_lsn_diff({bad}, '0/0')"));
        assert!(
            err.contains("invalid input syntax for type pg_lsn"),
            "{bad}: {err}"
        );
    }
    let err = error_of(&c, "SELECT pg_wal_lsn_diff('FFFFFFFF/FFFFFFFF', '0/0')");
    assert!(err.contains("unsupported capability"), "{err}");

    // Refused until they do the work their names promise.
    for sql in [
        "SELECT pg_current_wal_lsn()",
        "SELECT pg_wal_lsn_diff(pg_current_wal_lsn(), pg_current_wal_lsn())",
        "SELECT length(pg_current_wal_lsn()::text) > 0",
        "SELECT pg_export_snapshot()",
        "SELECT pg_notify('beyond_no_listener', 'fn-payload') IS NULL",
    ] {
        let err = error_of(&c, sql);
        assert!(err.contains("unsupported capability"), "{sql}: {err}");
    }

    // Partial: with NOTIFY refused, the queue is always empty.
    assert_eq!(
        row(
            &c,
            "SELECT pg_notification_queue_usage() >= 0::float AND pg_notification_queue_usage() <= 1::float"
        ),
        vec![t]
    );
    assert_eq!(
        row(&c, "SELECT length(repeat('x', 100))"),
        vec![SqlValue::Integer(100)]
    );
}

/// VALUES lists, CTEs, derived tables and views can be evaluated while a
/// statement is prepared, and a cached template is not prepared again. A
/// lock taken there would be taken once, at prepare, and never again; a
/// transaction id read there would come from no transaction. Those uses
/// are refused; the SELECT list, WHERE clause and INSERT values run per
/// execution.
#[test]
fn session_functions_refuse_to_run_while_a_statement_is_prepared() {
    let (_d, c) = open();
    let t = text("t");
    for sql in [
        "VALUES (pg_try_advisory_lock(31))",
        "SELECT * FROM (VALUES (pg_advisory_lock(31))) v",
        "WITH x AS (SELECT pg_try_advisory_lock(31) AS got) SELECT got FROM x",
        "SELECT got FROM (SELECT pg_try_advisory_lock(31) AS got) s",
        "VALUES (txid_current())",
    ] {
        let err = error_of(&c, sql);
        assert!(err.contains("unsupported capability"), "{sql}: {err}");
        assert!(
            err.contains("when the statement is prepared"),
            "{sql}: {err}"
        );
    }
    // Nothing above took key 31.
    assert_eq!(row(&c, "SELECT pg_advisory_unlock(31)"), vec![text("f")]);

    // The same functions run once per execution where they are evaluated
    // per row.
    c.execute("CREATE TABLE keys(k int)").expect("create");
    c.execute("INSERT INTO keys VALUES (32), (33)")
        .expect("insert");
    let mut lock = c
        .prepare("SELECT pg_try_advisory_lock(k) FROM keys ORDER BY k")
        .expect("prepare");
    for _ in 0..2 {
        let mut got = Vec::new();
        while let Step::Row = lock.step().expect("step") {
            got.push(lock.column_value(0).expect("value").clone());
        }
        assert_eq!(got, vec![t.clone(), t.clone()]);
        lock.reset().expect("reset");
    }
    for _ in 0..2 {
        assert_eq!(
            row(&c, "SELECT pg_advisory_unlock(32), pg_advisory_unlock(33)"),
            vec![t.clone(), t.clone()]
        );
    }
    assert_eq!(
        row(&c, "SELECT pg_advisory_unlock(32), pg_advisory_unlock(33)"),
        vec![text("f"), text("f")]
    );
    c.execute("CREATE TABLE ids(id bigint)")
        .expect("create ids");
    c.execute("BEGIN").expect("begin");
    c.execute("INSERT INTO ids VALUES (txid_current())")
        .expect("insert txid");
    assert_eq!(
        row(&c, "SELECT count(*) FROM ids WHERE id = txid_current()"),
        vec![SqlValue::Integer(1)]
    );
    assert_eq!(
        row(&c, "SELECT (SELECT txid_current()) = txid_current()"),
        vec![t.clone()]
    );
    c.execute("COMMIT").expect("commit");
}
