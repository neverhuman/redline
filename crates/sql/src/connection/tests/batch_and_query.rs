//! `execute_batch`, `query_map`, and the error for CREATE VIRTUAL TABLE
//! without a registered module.

use super::*;

#[test]
fn execute_batch_stops_on_error_and_preserves_previous_writes() {
    let (_dir, _db, conn) = new_db();

    conn.execute_batch(
        "CREATE TABLE t(id INTEGER PRIMARY KEY, v INTEGER DEFAULT 1);\
         INSERT INTO t(v) VALUES (10);\
         INSERT INTO t(id, v) VALUES (1, 20);\
         INSERT INTO t(v) VALUES (30);",
    )
    .expect_err("duplicate primary-key insert should fail before later statements");

    let mut after = conn
        .prepare("SELECT count(*) FROM t")
        .expect("query row count statement");
    assert_eq!(after.step().expect("step row"), Step::Row);
    assert_eq!(after.column_i64(0).expect("count"), 1);
    assert_eq!(after.step().expect("step done"), Step::Done);
}

#[test]
fn execute_batch_executes_explicit_transactions() {
    let (_dir, _db, conn) = new_db();

    conn.execute_batch(
        "CREATE TABLE t(id INTEGER PRIMARY KEY);\
         BEGIN IMMEDIATE;\
         INSERT INTO t VALUES (1);\
         COMMIT;\
         SELECT COUNT(*) FROM t",
    )
    .expect("batched tx script");

    let mut count = conn
        .prepare("SELECT COUNT(*) FROM t")
        .expect("count prepared");
    assert_eq!(count.step().expect("count step"), Step::Row);
    assert_eq!(count.column_i64(0).expect("count value"), 1);
}

#[test]
fn query_map_maps_rows_and_preserves_order_after_callback_error() {
    let (_dir, _db, conn) = new_db();

    conn.execute_batch(
        "CREATE TABLE t(id INTEGER PRIMARY KEY, v INTEGER); INSERT INTO t(v) VALUES (1), (2), (3)",
    )
    .expect("setup");

    let mut stmt = conn
        .prepare("SELECT v FROM t ORDER BY v")
        .expect("prepared select");
    let mut rows = stmt.query_map(|row| {
        let value = row.column_i64(0)?;
        if value == 2 {
            Err(Error::UnsupportedSql("mapped stop".to_owned()))
        } else {
            Ok(value)
        }
    });

    assert_eq!(rows.next().expect("first"), Ok(1));
    assert_eq!(
        rows.next().expect("second"),
        Err(Error::UnsupportedSql("mapped stop".to_owned()))
    );
    assert_eq!(rows.next().expect("third"), Ok(3));
    assert!(rows.next().is_none());
}

#[test]
fn create_virtual_table_is_unsupported_without_module_migration() {
    let (_dir, _db, conn) = new_db();

    let err = conn.execute("CREATE VIRTUAL TABLE boxes USING nosuch (id, x1, x2, y1, y2)");
    match err {
        Ok(v) => panic!("expected failure, got ok({v:?})"),
        Err(Error::UnsupportedSql(message)) => assert!(
            message.contains("CREATE VIRTUAL TABLE"),
            "unexpected unsupported message: {message}"
        ),
        Err(other) => panic!("unexpected create virtual table error: {other:?}"),
    }

    let mut probe = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='boxes'")
        .expect("probe prepared");
    assert_eq!(probe.step().expect("probe step"), Step::Done);
}
