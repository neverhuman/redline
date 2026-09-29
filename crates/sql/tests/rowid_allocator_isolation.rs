//! Each table allocates its own rowids, as SQLite does.
//!
//! RedlineDB used one engine-wide rowid counter, and a DELETE from a table
//! with an INTEGER PRIMARY KEY lowered it to that table's maximum rowid + 1.
//! The next insert into any other table then took a rowid that table already
//! used, and the new row silently replaced the old one. Each expectation
//! below is what the pinned sqlite3 3.53.1 shell prints for the same script.

use std::sync::Arc;

use redlinedb_sql::{Connection, Database, DbOptions, Step};

fn rows(conn: &Arc<Connection>, sql: &str) -> Vec<(i64, i64)> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let mut out = Vec::new();
    while stmt.step().expect("step") == Step::Row {
        out.push((
            stmt.column_i64(0).expect("column 0"),
            stmt.column_i64(1).expect("column 1"),
        ));
    }
    out
}

fn open(dir: &tempfile::TempDir) -> Arc<Connection> {
    let path = dir.path().join("rowid-allocator.db");
    let db = if path.exists() {
        Database::open(&path, DbOptions::default()).expect("open database")
    } else {
        Database::create(&path, DbOptions::default()).expect("create database")
    };
    db.connect()
}

#[test]
fn a_delete_in_one_table_never_hands_out_another_tables_rowid() {
    let dir = tempfile::tempdir().expect("temp dir");
    let conn = open(&dir);
    conn.execute_batch(
        "CREATE TABLE b(x);
         INSERT INTO b VALUES (1), (2), (3);
         CREATE TABLE a(id INTEGER PRIMARY KEY);
         INSERT INTO a VALUES (NULL);
         DELETE FROM a;
         INSERT INTO b VALUES (99);",
    )
    .expect("script");
    assert_eq!(
        rows(&conn, "SELECT rowid, x FROM b"),
        [(1, 1), (2, 2), (3, 3), (4, 99)]
    );
}

#[test]
fn every_table_numbers_its_rows_from_one() {
    let dir = tempfile::tempdir().expect("temp dir");
    let conn = open(&dir);
    conn.execute_batch(
        "CREATE TABLE t1(x);
         INSERT INTO t1 VALUES (10), (20);
         CREATE TABLE t2(x);
         INSERT INTO t2 VALUES (30), (40);",
    )
    .expect("script");
    assert_eq!(rows(&conn, "SELECT rowid, x FROM t2"), [(1, 30), (2, 40)]);
}

#[test]
fn integer_primary_keys_reuse_only_their_own_deleted_maximum() {
    let dir = tempfile::tempdir().expect("temp dir");
    let conn = open(&dir);
    conn.execute_batch(
        "CREATE TABLE a(id INTEGER PRIMARY KEY, v);
         INSERT INTO a(v) VALUES (1), (2), (3);
         CREATE TABLE b(id INTEGER PRIMARY KEY, v);
         INSERT INTO b(v) VALUES (7);
         DELETE FROM a WHERE id = 3;
         INSERT INTO b(v) VALUES (8);
         INSERT INTO a(v) VALUES (4);",
    )
    .expect("script");
    assert_eq!(rows(&conn, "SELECT id, v FROM a"), [(1, 1), (2, 2), (3, 4)]);
    assert_eq!(rows(&conn, "SELECT id, v FROM b"), [(1, 7), (2, 8)]);
}

#[test]
fn rowids_stay_per_table_after_reopen() {
    let dir = tempfile::tempdir().expect("temp dir");
    {
        let conn = open(&dir);
        conn.execute_batch(
            "CREATE TABLE b(x);
             INSERT INTO b VALUES (1), (2), (3);
             CREATE TABLE a(id INTEGER PRIMARY KEY);
             INSERT INTO a VALUES (NULL);",
        )
        .expect("first session");
    }
    let conn = open(&dir);
    conn.execute_batch(
        "DELETE FROM a;
         INSERT INTO b VALUES (99);
         INSERT INTO a VALUES (NULL);",
    )
    .expect("second session");
    assert_eq!(
        rows(&conn, "SELECT rowid, x FROM b"),
        [(1, 1), (2, 2), (3, 3), (4, 99)]
    );
    let mut stmt = conn.prepare("SELECT id FROM a").expect("prepare");
    assert_eq!(stmt.step().expect("step"), Step::Row);
    assert_eq!(stmt.column_i64(0).expect("id"), 1);
}

#[test]
fn a_rolled_back_delete_of_the_highest_rowid_leaves_that_rowid_taken() {
    let dir = tempfile::tempdir().expect("temp dir");
    let conn = open(&dir);
    conn.execute_batch(
        "CREATE TABLE a(id INTEGER PRIMARY KEY, v);
         INSERT INTO a(v) VALUES (1), (2), (3);
         BEGIN;
         DELETE FROM a WHERE id = 3;
         ROLLBACK;
         INSERT INTO a(v) VALUES (9);",
    )
    .expect("script");
    assert_eq!(
        rows(&conn, "SELECT id, v FROM a"),
        [(1, 1), (2, 2), (3, 3), (4, 9)]
    );
}
