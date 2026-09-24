//! Locks row order the official stdout diff depends on.
//!
//! The nested-loop join sequence and the `LIMIT 1` prefix stay as they
//! are. A later hash join or early-stop scan has to update this file
//! on purpose. These tests do not switch either path on.

mod common;

use std::sync::Arc;

use common::open_database;
use redlinedb_sql::{Connection, SqlValue, Step};

fn query_pairs(conn: &Arc<Connection>, sql: &str) -> Vec<(SqlValue, SqlValue)> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let mut rows = Vec::new();
    loop {
        match stmt.step().expect("step") {
            Step::Row => rows.push((
                stmt.column_value(0).expect("left").clone(),
                stmt.column_value(1).expect("right").clone(),
            )),
            Step::Done => break,
        }
    }
    rows
}

#[test]
fn unindexed_inner_equijoin_keeps_nested_loop_order() {
    let (_dir, conn) = open_database();
    conn.execute("CREATE TABLE a(x INTEGER, y INTEGER)")
        .expect("create a");
    conn.execute("CREATE TABLE b(x INTEGER, y INTEGER)")
        .expect("create b");
    conn.execute("INSERT INTO a(x, y) VALUES (1, 10), (1, 11), (NULL, 12), (2, 13)")
        .expect("insert a");
    conn.execute("INSERT INTO b(x, y) VALUES (1, 20), (NULL, 21), (2, 22), (1, 23)")
        .expect("insert b");

    let rows = query_pairs(&conn, "SELECT a.y, b.y FROM a JOIN b ON a.x = b.x");
    let n = |v: i64| SqlValue::Integer(v);
    assert_eq!(
        rows,
        vec![
            (n(10), n(20)),
            (n(10), n(23)),
            (n(11), n(20)),
            (n(11), n(23)),
            (n(13), n(22)),
        ]
    );
}

#[test]
fn limit_one_matches_the_first_full_scan_row() {
    let (_dir, conn) = open_database();
    conn.execute("CREATE TABLE t(id INTEGER, v INTEGER)")
        .expect("create");
    conn.execute("INSERT INTO t(id, v) VALUES (1, 10), (2, NULL), (3, 30)")
        .expect("insert");
    let full = query_pairs(&conn, "SELECT id, v FROM t");
    let limited = query_pairs(&conn, "SELECT id, v FROM t LIMIT 1");
    assert!(
        full.iter()
            .any(|(_, value)| matches!(value, SqlValue::Null)),
        "table must contain a null so the scan is not all integers"
    );
    assert_eq!(limited, vec![full[0].clone()]);

    conn.execute("CREATE TABLE n(id INTEGER, v INTEGER)")
        .expect("create n");
    conn.execute("INSERT INTO n(id, v) VALUES (NULL, NULL), (2, 5)")
        .expect("insert n");
    let null_full = query_pairs(&conn, "SELECT id, v FROM n");
    let null_limit = query_pairs(&conn, "SELECT id, v FROM n LIMIT 1");
    assert!(matches!(null_full[0].0, SqlValue::Null));
    assert_eq!(null_limit, vec![null_full[0].clone()]);
}
