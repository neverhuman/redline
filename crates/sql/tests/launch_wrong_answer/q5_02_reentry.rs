//! Q5-02 follow-up: a row that leaves an index and comes back with the same
//! key and rowid is in the index again.
//!
//! The index entry is `(key, rowid)`, so a row that re-enters an index under
//! the key it left with has the physical bytes of its own tombstone. The
//! kernel insert found that dead cell, took it for the entry, and did
//! nothing: after `flag 1 -> 0 -> 1` the row was missing from its partial
//! index, reads through the index lost it, and a partial UNIQUE index let a
//! second row in under the same key. Plain indexes lost the entry the same
//! way on a key round trip `A -> B -> A` and on DELETE followed by an INSERT
//! of the same rowid and key.
//!
//! Every case is compared with bundled SQLite through the index, through a
//! scan and through the planner, and checked against the index's B-tree and
//! `PRAGMA integrity_check`.

use std::sync::Arc;

use redlinedb_sql::{Connection, SqlValue, Step};

use crate::lab::{Lab, int};
use crate::q5_02_partial_update::{
    T, U, assert_integrity_ok, assert_members, check_reads, check_t, check_u,
};

/// `w`: a plain index and a plain UNIQUE index over a rowid-alias table.
const W: &str = "CREATE TABLE w(id INTEGER PRIMARY KEY, k INTEGER, v TEXT); \
    CREATE INDEX w_k ON w(k); \
    CREATE UNIQUE INDEX w_v ON w(v);";

fn check_w(lab: &Lab, keys: &[i64]) {
    check_reads(lab, "w", "w_k", "1", keys);
    for access in ["w INDEXED BY w_v", "w NOT INDEXED", "w"] {
        for v in ["'a'", "'b'", "'c'"] {
            lab.assert_same(&format!("SELECT id FROM {access} WHERE v = {v}"), false);
        }
        lab.assert_same(
            &format!("SELECT v FROM {access} WHERE v > '' ORDER BY v"),
            true,
        );
    }
    assert_members(lab, "w_k", "w", "1");
    assert_members(lab, "w_v", "w", "v IS NOT NULL");
    assert_integrity_ok(lab);
}

fn rows(conn: &Arc<Connection>, sql: &str) -> Vec<Vec<SqlValue>> {
    let mut stmt = conn
        .prepare(sql)
        .unwrap_or_else(|err| panic!("{sql}: {err}"));
    let mut out = Vec::new();
    while stmt.step().unwrap_or_else(|err| panic!("{sql}: {err}")) == Step::Row {
        out.push(
            (0..stmt.column_count())
                .map(|i| stmt.column_value(i).expect("value").clone())
                .collect(),
        );
    }
    out
}

#[test]
fn q5_02_toggle_back_into_a_partial_index() {
    let lab = Lab::new();
    lab.exec_both(T);
    lab.exec_both(U);
    lab.exec_both("INSERT INTO t VALUES (1, 100, 1), (2, 200, 1)");
    lab.exec_both("INSERT INTO u VALUES (1, 5, 1)");
    for round in 0..3 {
        lab.exec_both("UPDATE t SET flag = 0 WHERE id = 1");
        lab.exec_both("UPDATE u SET flag = 0 WHERE id = 1");
        check_t(&lab, &[100, 200]);
        check_u(&lab, &[5]);
        lab.exec_both("UPDATE t SET flag = 1 WHERE id = 1");
        lab.exec_both("UPDATE u SET flag = 1 WHERE id = 1");
        lab.assert_rows(
            "SELECT id FROM t INDEXED BY ix_k WHERE flag = 1 AND k = 100",
            &[vec![int(1)]],
        );
        lab.assert_rows(
            "SELECT id FROM u INDEXED BY ux WHERE flag = 1 AND k = 5",
            &[vec![int(1)]],
        );
        check_t(&lab, &[100, 200]);
        check_u(&lab, &[5]);
        // Row 1 holds key 5 again, so a second member is refused.
        lab.assert_error(
            &format!("INSERT INTO u VALUES ({}, 5, 1)", 10 + round),
            "unique constraint",
        );
        check_u(&lab, &[5]);
    }
}

#[test]
fn q5_02_toggle_inside_one_transaction() {
    let lab = Lab::new();
    lab.exec_both(T);
    lab.exec_both(U);
    lab.exec_both("INSERT INTO t VALUES (1, 100, 1), (2, 200, 0)");
    lab.exec_both("INSERT INTO u VALUES (1, 5, 1)");
    lab.exec_both("BEGIN");
    lab.exec_both("UPDATE t SET flag = 0 WHERE id = 1");
    lab.exec_both("UPDATE t SET flag = 1 WHERE id = 1");
    lab.exec_both("UPDATE t SET flag = 1 WHERE id = 2");
    lab.exec_both("UPDATE t SET flag = 0 WHERE id = 2");
    lab.exec_both("UPDATE t SET flag = 1 WHERE id = 2");
    lab.exec_both("UPDATE u SET flag = 0 WHERE id = 1");
    lab.exec_both("UPDATE u SET flag = 1 WHERE id = 1");
    check_reads(&lab, "t", "ix_k", "flag = 1", &[100, 200]);
    check_reads(&lab, "u", "ux", "flag = 1", &[5]);
    // (A failed statement would end RedlineDB's transaction, so uniqueness
    // is checked after COMMIT.)
    lab.exec_both("COMMIT");
    check_t(&lab, &[100, 200]);
    check_u(&lab, &[5]);
    lab.assert_error("INSERT INTO u VALUES (2, 5, 1)", "unique constraint");
    check_u(&lab, &[5]);

    // The same toggles rolled back leave the committed membership.
    lab.exec_both("BEGIN");
    lab.exec_both("UPDATE t SET flag = 0 WHERE id = 1");
    lab.exec_both("UPDATE t SET flag = 1 WHERE id = 1");
    lab.exec_both("UPDATE t SET flag = 0 WHERE id = 1");
    lab.exec_both("UPDATE u SET flag = 0 WHERE id = 1");
    lab.exec_both("UPDATE u SET flag = 1 WHERE id = 1");
    lab.exec_both("ROLLBACK");
    check_t(&lab, &[100, 200]);
    check_u(&lab, &[5]);
    lab.assert_error("INSERT INTO u VALUES (2, 5, 1)", "unique constraint");
}

#[test]
fn q5_02_enter_roll_back_and_enter_again() {
    let lab = Lab::new();
    lab.exec_both(T);
    lab.exec_both(U);
    lab.exec_both("INSERT INTO t VALUES (1, 100, 1), (2, 200, 0)");
    lab.exec_both("INSERT INTO u VALUES (1, 5, 0)");
    lab.exec_both("BEGIN");
    lab.exec_both("UPDATE t SET flag = 1 WHERE id = 2");
    lab.exec_both("UPDATE u SET flag = 1 WHERE id = 1");
    lab.exec_both("ROLLBACK");
    check_t(&lab, &[100, 200]);
    check_u(&lab, &[5]);
    lab.exec_both("UPDATE t SET flag = 1 WHERE id = 2");
    lab.exec_both("UPDATE u SET flag = 1 WHERE id = 1");
    lab.assert_rows(
        "SELECT id FROM t INDEXED BY ix_k WHERE flag = 1 AND k = 200",
        &[vec![int(2)]],
    );
    check_t(&lab, &[100, 200]);
    check_u(&lab, &[5]);
    lab.assert_error("INSERT INTO u VALUES (2, 5, 1)", "unique constraint");
    check_u(&lab, &[5]);
}

#[test]
fn q5_02_plain_index_key_round_trip() {
    let lab = Lab::new();
    lab.exec_both(W);
    lab.exec_both("INSERT INTO w VALUES (1, 10, 'a'), (2, 20, 'b')");
    // Committed A -> B -> A.
    lab.exec_both("UPDATE w SET k = 11, v = 'c' WHERE id = 1");
    lab.exec_both("UPDATE w SET k = 10, v = 'a' WHERE id = 1");
    check_w(&lab, &[10, 11, 20]);
    lab.assert_error("INSERT INTO w VALUES (3, 30, 'a')", "unique constraint");
    // A -> B -> A inside one transaction, then committed.
    lab.exec_both("BEGIN");
    lab.exec_both("UPDATE w SET k = 21, v = 'c' WHERE id = 2");
    lab.exec_both("UPDATE w SET k = 20, v = 'b' WHERE id = 2");
    check_reads(&lab, "w", "w_k", "1", &[20, 21]);
    lab.exec_both("COMMIT");
    check_w(&lab, &[10, 20, 21]);
    lab.assert_error("INSERT INTO w VALUES (3, 30, 'b')", "unique constraint");
}

#[test]
fn q5_02_delete_then_insert_the_same_row() {
    let lab = Lab::new();
    lab.exec_both(W);
    lab.exec_both("INSERT INTO w VALUES (1, 10, 'a'), (2, 20, 'b')");
    lab.exec_both("DELETE FROM w WHERE id = 1");
    lab.exec_both("INSERT INTO w VALUES (1, 10, 'a')");
    check_w(&lab, &[10, 20]);
    lab.assert_error("INSERT INTO w VALUES (3, 30, 'a')", "unique constraint");
    lab.exec_both("BEGIN");
    lab.exec_both("DELETE FROM w WHERE id = 2");
    lab.exec_both("INSERT INTO w VALUES (2, 20, 'b')");
    lab.exec_both("COMMIT");
    check_w(&lab, &[10, 20]);
    lab.exec_both("REPLACE INTO w VALUES (2, 20, 'b')");
    check_w(&lab, &[10, 20]);
    lab.assert_error("INSERT INTO w VALUES (3, 30, 'b')", "unique constraint");
}

const REENTRY_SCRIPT: &str = "INSERT INTO t VALUES (1, 100, 1), (2, 200, 0); \
    INSERT INTO u VALUES (1, 5, 1); \
    INSERT INTO w VALUES (1, 10, 'a'), (2, 20, 'b'); \
    UPDATE t SET flag = 0 WHERE id = 1; UPDATE t SET flag = 1 WHERE id = 1; \
    UPDATE u SET flag = 0 WHERE id = 1; UPDATE u SET flag = 1 WHERE id = 1; \
    UPDATE w SET k = 11 WHERE id = 1; UPDATE w SET k = 10 WHERE id = 1; \
    DELETE FROM w WHERE id = 2; INSERT INTO w VALUES (2, 20, 'b'); \
    UPDATE t SET flag = 0 WHERE id = 1; UPDATE t SET flag = 1 WHERE id = 1; \
    BEGIN; UPDATE u SET flag = 0 WHERE id = 1; UPDATE u SET flag = 1 WHERE id = 1; COMMIT; \
    BEGIN; UPDATE t SET flag = 1 WHERE id = 2; ROLLBACK; UPDATE t SET flag = 1 WHERE id = 2;";

/// Run the re-entry script, then open the database again: after a clean
/// close, or (`crash`) without closing it, so the open replays the WAL.
fn reentry_after_reopen(crash: bool) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("reentry.db");
    let script = REENTRY_SCRIPT;
    {
        let lab = Lab::open(&path);
        lab.exec_both(&format!("{T} {U} {W}"));
        for statement in script.split(';').filter(|s| !s.trim().is_empty()) {
            lab.exec_both(statement);
        }
        check_t(&lab, &[100, 200]);
        check_u(&lab, &[5]);
        check_w(&lab, &[10, 11, 20]);
        if crash {
            // No close and no checkpoint: the next open recovers.
            std::mem::forget(lab);
        }
    }
    let lab = Lab::open(&path);
    lab.sqlite
        .execute_batch(&format!("{T} {U} {W} {script}"))
        .expect("sqlite replay");
    check_t(&lab, &[100, 200]);
    check_u(&lab, &[5]);
    check_w(&lab, &[10, 11, 20]);
    lab.assert_error("INSERT INTO u VALUES (2, 5, 1)", "unique constraint");
}

#[test]
fn q5_02_reentry_survives_reopen() {
    reentry_after_reopen(false);
}

#[test]
fn q5_02_reentry_survives_recovery() {
    reentry_after_reopen(true);
}

/// A snapshot that began while the row was a member keeps reading it
/// through the index after another connection moves it out and back in, and
/// a snapshot that began while it was out keeps not reading it.
#[test]
fn q5_02_reentry_keeps_older_snapshots() {
    let lab = Lab::new();
    lab.exec_both(T);
    lab.exec_both("INSERT INTO t VALUES (1, 100, 1), (2, 200, 1)");
    let indexed = "SELECT id FROM t INDEXED BY ix_k WHERE flag = 1 AND k > 0 ORDER BY id";
    let scan = "SELECT id FROM t NOT INDEXED WHERE flag = 1 AND k > 0 ORDER BY id";
    let both = vec![vec![int(1)], vec![int(2)]];
    let second = vec![vec![int(2)]];
    let reader = lab.database.connect();
    reader.execute("BEGIN").expect("begin");
    assert_eq!(rows(&reader, indexed), both);
    lab.exec_both("UPDATE t SET flag = 0 WHERE id = 1");
    let between = lab.database.connect();
    between.execute("BEGIN").expect("begin");
    assert_eq!(rows(&between, indexed), second);
    lab.exec_both("UPDATE t SET flag = 1 WHERE id = 1");
    for sql in [indexed, scan] {
        assert_eq!(rows(&reader, sql), both, "{sql}");
        assert_eq!(rows(&between, sql), second, "{sql}");
        assert_eq!(rows(&lab.redline, sql), both, "{sql}");
    }
    reader.execute("COMMIT").expect("commit");
    between.execute("COMMIT").expect("commit");
    for sql in [indexed, scan] {
        assert_eq!(rows(&reader, sql), both, "{sql}");
        assert_eq!(rows(&between, sql), both, "{sql}");
    }
    check_t(&lab, &[100, 200]);
}
