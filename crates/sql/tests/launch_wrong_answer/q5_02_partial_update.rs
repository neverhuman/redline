//! Q5-02: an UPDATE keeps partial-index membership in step with the heap.
//!
//! `maintain_indexes_on_update` never evaluated a partial index's WHERE
//! clause. A row whose UPDATE made the predicate true (false -> true) was not
//! added to the index, a row whose UPDATE made it false (true -> false) kept
//! its entry, and a row outside the index whose key changed was added to it.
//! Reads through the index then missed rows, a partial UNIQUE index raised
//! false conflicts or admitted duplicates, and `PRAGMA integrity_check`
//! answered `ok`.
//!
//! Every test compares the rows through the index (`INDEXED BY`), through a
//! scan (`NOT INDEXED`) and through the planner's own choice with bundled
//! SQLite, and reads the index's B-tree directly to check that it holds
//! exactly the rows whose predicate is true.

use std::ops::Bound;
use std::path::Path;
use std::sync::Arc;

use redlinedb_kernel::catalog::with_v4_index_format_for_tests;
use redlinedb_kernel::engine::CommitOutcome;
use redlinedb_kernel::format::RowId;
use redlinedb_kernel::index::{CursorYield, IndexCursor, KeyRange, SnapshotView};
use redlinedb_kernel::txn::Isolation;
use redlinedb_sql::{Connection, Database, DbOptions, SqlValue};

use crate::lab::{Lab, Outcome, int, text};

/// `t`: a partial index over a rowid-alias table.
const T: &str = "CREATE TABLE t(id INTEGER PRIMARY KEY, k INTEGER, flag INTEGER); \
    CREATE INDEX ix_k ON t(k) WHERE flag = 1;";

/// `u`: a partial UNIQUE index.
const U: &str = "CREATE TABLE u(id INTEGER PRIMARY KEY, k INTEGER, flag INTEGER); \
    CREATE UNIQUE INDEX ux ON u(k) WHERE flag = 1;";

/// Reads of a table with a partial index on `k` whose predicate is `pred`.
/// `{access}` is replaced by the table with an access hint.
fn reads(pred: &str, keys: &[i64]) -> Vec<(String, bool)> {
    let mut out = vec![
        (
            format!("SELECT id FROM {{access}} WHERE {pred} AND k > 0 ORDER BY id"),
            true,
        ),
        (
            format!("SELECT k FROM {{access}} WHERE {pred} AND k >= 0 ORDER BY k"),
            true,
        ),
        (
            format!("SELECT count(*) FROM {{access}} WHERE {pred} AND k > 0"),
            true,
        ),
    ];
    for k in keys {
        out.push((
            format!("SELECT id, k FROM {{access}} WHERE {pred} AND k = {k}"),
            false,
        ));
    }
    out
}

/// Each read must agree with SQLite through `index`, through a scan and
/// through the planner's own choice.
fn check_reads(lab: &Lab, table: &str, index: &str, pred: &str, keys: &[i64]) {
    for (query, ordered) in reads(pred, keys) {
        for access in [
            format!("{table} INDEXED BY {index}"),
            format!("{table} NOT INDEXED"),
            table.to_owned(),
        ] {
            lab.assert_same(&query.replace("{access}", &access), ordered);
        }
    }
}

/// The rowids index `index` holds, sorted, read from its B-tree at the
/// latest committed snapshot.
fn index_rowids(conn: &Connection, index: &str) -> Vec<i64> {
    let engine = conn.engine_for_tests();
    let schema = engine.schema_snapshot();
    let def = schema
        .indexes
        .iter()
        .find(|def| def.name.as_ref() == index)
        .unwrap_or_else(|| panic!("no index {index}"));
    let handle = engine.index_handle(def.index_id).expect("index handle");
    let latest = engine.tx_status().snapshot();
    let view = SnapshotView::visible(engine.tx_status(), &latest, None);
    let range = KeyRange {
        start: Bound::Unbounded,
        end: Bound::Unbounded,
    };
    let mut cursor = IndexCursor::open(&handle, range, view).expect("cursor");
    let mut rows = Vec::new();
    while let CursorYield::Batch(_) = cursor.next_batch(&mut rows, 64).expect("batch") {}
    let mut ids: Vec<i64> = rows.iter().map(|row| row.row_id.0 as i64).collect();
    ids.sort_unstable();
    ids
}

/// Index `index` on `table` must hold exactly the rows whose `pred` is true:
/// the rowids of a scan of the heap, whose rows in turn agree with SQLite.
/// (RedlineDB numbers the rows of a table without a rowid alias from one
/// counter shared by all tables, so rowids are compared on this side only.)
fn assert_members(lab: &Lab, index: &str, table: &str, pred: &str) {
    lab.assert_same(
        &format!("SELECT id FROM {table} NOT INDEXED WHERE {pred} ORDER BY id"),
        true,
    );
    let heap = format!("SELECT rowid FROM {table} NOT INDEXED WHERE {pred} ORDER BY rowid");
    let Outcome::Rows(rows) = lab.rows_redline(&heap, &[]) else {
        panic!("redline failed `{heap}`");
    };
    let want: Vec<i64> = rows
        .iter()
        .map(|row| match row[0] {
            SqlValue::Integer(id) => id,
            ref other => panic!("rowid {other:?}"),
        })
        .collect();
    assert_eq!(
        index_rowids(&lab.redline, index),
        want,
        "index {index} does not hold exactly the rows of `{heap}`"
    );
}

/// The rowids of `table`, in order.
fn rowids(conn: &Arc<Connection>, table: &str) -> Vec<u64> {
    let mut stmt = conn
        .prepare(&format!("SELECT rowid FROM {table} ORDER BY rowid"))
        .expect("prepare");
    let mut out = Vec::new();
    while stmt.step().expect("step") == redlinedb_sql::Step::Row {
        match stmt.column_value(0).expect("value") {
            SqlValue::Integer(rowid) => out.push(*rowid as u64),
            other => panic!("rowid {other:?}"),
        }
    }
    out
}

fn integrity_rows(conn: &Arc<Connection>) -> Vec<String> {
    let mut stmt = conn.prepare("PRAGMA integrity_check").expect("prepare");
    let mut out = Vec::new();
    while stmt.step().expect("step") == redlinedb_sql::Step::Row {
        match stmt.column_value(0).expect("value") {
            SqlValue::Text(line) => out.push(line.to_string()),
            other => panic!("integrity_check row {other:?}"),
        }
    }
    out
}

fn assert_integrity_ok(lab: &Lab) {
    assert_eq!(integrity_rows(&lab.redline), vec!["ok".to_owned()]);
}

fn check_t(lab: &Lab, keys: &[i64]) {
    check_reads(lab, "t", "ix_k", "flag = 1", keys);
    assert_members(lab, "ix_k", "t", "flag = 1");
    assert_integrity_ok(lab);
}

fn check_u(lab: &Lab, keys: &[i64]) {
    check_reads(lab, "u", "ux", "flag = 1", keys);
    assert_members(lab, "ux", "u", "flag = 1");
    assert_integrity_ok(lab);
}

#[test]
fn q5_02_false_to_true_point() {
    let lab = Lab::new();
    lab.exec_both(T);
    lab.exec_both("INSERT INTO t VALUES (1, 100, 1), (2, 200, 0), (3, 300, 1)");
    lab.exec_both("UPDATE t SET flag = 1 WHERE id = 2");
    lab.assert_rows(
        "SELECT id FROM t INDEXED BY ix_k WHERE flag = 1 AND k = 200",
        &[vec![int(2)]],
    );
    lab.assert_rows(
        "SELECT count(*) FROM t INDEXED BY ix_k WHERE flag = 1 AND k > 0",
        &[vec![int(3)]],
    );
    check_t(&lab, &[100, 200, 300]);
    // A bound key reads the same row.
    lab.assert_same_bound(
        "SELECT id FROM t INDEXED BY ix_k WHERE flag = 1 AND k = ?1",
        &[int(200)],
        false,
    );
}

#[test]
fn q5_02_true_to_false_unique_no_false_conflict() {
    let lab = Lab::new();
    lab.exec_both(U);
    lab.exec_both("INSERT INTO u VALUES (1, 5, 1)");
    lab.exec_both("UPDATE u SET flag = 0 WHERE id = 1");
    // Row 1 left the index, so key 5 is free for a new member.
    lab.step_both("INSERT INTO u VALUES (2, 5, 1)");
    lab.assert_rows(
        "SELECT id FROM u WHERE k = 5 ORDER BY id",
        &[vec![int(1)], vec![int(2)]],
    );
    lab.assert_rows(
        "SELECT id FROM u INDEXED BY ux WHERE flag = 1 AND k = 5",
        &[vec![int(2)]],
    );
    check_u(&lab, &[5]);
    // Moving row 1 back in now collides with row 2.
    lab.assert_error("UPDATE u SET flag = 1 WHERE id = 1", "unique constraint");
    check_u(&lab, &[5]);
}

#[test]
fn q5_02_nonmember_key_change_no_phantom() {
    let lab = Lab::new();
    lab.exec_both(U);
    lab.exec_both("INSERT INTO u VALUES (3, 7, 0)");
    // Row 3 is outside the index; changing its key must not add it.
    lab.exec_both("UPDATE u SET k = 8 WHERE id = 3");
    lab.step_both("INSERT INTO u VALUES (4, 8, 1)");
    lab.assert_rows(
        "SELECT id FROM u WHERE k = 8 ORDER BY id",
        &[vec![int(3)], vec![int(4)]],
    );
    lab.assert_rows(
        "SELECT id FROM u INDEXED BY ux WHERE flag = 1 AND k = 8",
        &[vec![int(4)]],
    );
    check_u(&lab, &[7, 8]);
    // The same through a prepared statement with bound values.
    lab.exec_both("INSERT INTO u VALUES (5, 20, 0)");
    lab.assert_same_bound(
        "UPDATE u SET k = ?1 WHERE id = ?2",
        &[int(21), int(5)],
        true,
    );
    lab.step_both("INSERT INTO u VALUES (6, 21, 1)");
    check_u(&lab, &[7, 8, 20, 21]);
}

#[test]
fn q5_02_false_to_true_unique_enforced() {
    let lab = Lab::new();
    lab.exec_both(U);
    lab.exec_both("INSERT INTO u VALUES (5, 9, 0)");
    lab.exec_both("UPDATE u SET flag = 1 WHERE id = 5");
    // Row 5 entered the index with key 9, so a second member is refused.
    lab.assert_error("INSERT INTO u VALUES (6, 9, 1)", "unique constraint");
    lab.step_both("INSERT OR IGNORE INTO u VALUES (6, 9, 1)");
    lab.exec_both("INSERT INTO u VALUES (7, 9, 0)");
    lab.assert_error("UPDATE u SET flag = 1 WHERE id = 7", "unique constraint");
    lab.assert_rows(
        "SELECT id, flag FROM u WHERE k = 9 ORDER BY id",
        &[vec![int(5), int(1)], vec![int(7), int(0)]],
    );
    check_u(&lab, &[9]);
    // An upsert whose DO UPDATE moves the conflicting row out of the index
    // lets the key be reused.
    lab.step_both("INSERT INTO u VALUES (5, 9, 1) ON CONFLICT(id) DO UPDATE SET flag = 0");
    lab.step_both("INSERT INTO u VALUES (8, 9, 1)");
    lab.assert_rows(
        "SELECT id FROM u INDEXED BY ux WHERE flag = 1 AND k = 9",
        &[vec![int(8)]],
    );
    check_u(&lab, &[9]);
}

#[test]
fn q5_02_rollback_restores_membership() {
    let lab = Lab::new();
    lab.exec_both(T);
    lab.exec_both(U);
    lab.exec_both("INSERT INTO t VALUES (1, 100, 1), (2, 200, 0)");
    lab.exec_both("INSERT INTO u VALUES (1, 5, 1), (2, 6, 0)");
    lab.exec_both("BEGIN");
    lab.exec_both("UPDATE t SET flag = 1 WHERE id = 2");
    lab.exec_both("UPDATE t SET flag = 0 WHERE id = 1");
    lab.exec_both("UPDATE u SET flag = 0 WHERE id = 1");
    lab.exec_both("UPDATE u SET flag = 1 WHERE id = 2");
    // Inside the transaction the index answers with its own changes.
    check_reads(&lab, "t", "ix_k", "flag = 1", &[100, 200]);
    check_reads(&lab, "u", "ux", "flag = 1", &[5, 6]);
    lab.exec_both("ROLLBACK");
    check_t(&lab, &[100, 200]);
    check_u(&lab, &[5, 6]);
    // Row 1 is a member of `ux` again and row 2 is not.
    lab.assert_error("INSERT INTO u VALUES (3, 5, 1)", "unique constraint");
    lab.step_both("INSERT INTO u VALUES (4, 6, 1)");
    check_u(&lab, &[5, 6]);
}

/// The transitions of the tests above, run once and then read back after
/// the database is closed and opened again.
const TRANSITIONS: &str = "INSERT INTO t VALUES (1, 100, 1), (2, 200, 0), (3, 300, 1), (4, 400, 0); \
    UPDATE t SET flag = 1 WHERE id = 2; \
    UPDATE t SET flag = 0 WHERE id = 3; \
    UPDATE t SET k = 401 WHERE id = 4; \
    UPDATE t SET k = 101 WHERE id = 1; \
    INSERT INTO u VALUES (1, 5, 1), (2, 6, 0), (3, 7, 0); \
    UPDATE u SET flag = 0 WHERE id = 1; \
    UPDATE u SET flag = 1 WHERE id = 2; \
    UPDATE u SET k = 5 WHERE id = 3; \
    INSERT INTO u VALUES (4, 5, 1);";

#[test]
fn q5_02_reopen_persists() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("q5_02.db");
    {
        let lab = Lab::open(&path);
        lab.exec_both(T);
        lab.exec_both(U);
        lab.exec_both(TRANSITIONS);
        check_t(&lab, &[100, 101, 200, 300, 400, 401]);
        check_u(&lab, &[5, 6, 7]);
    }
    let lab = Lab::open(&path);
    lab.sqlite
        .execute_batch(&format!("{T} {U} {TRANSITIONS}"))
        .expect("sqlite replay");
    check_t(&lab, &[100, 101, 200, 300, 400, 401]);
    check_u(&lab, &[5, 6, 7]);
    lab.assert_error("INSERT INTO u VALUES (9, 6, 1)", "unique constraint");
    lab.step_both("INSERT INTO u VALUES (9, 7, 1)");
    check_u(&lab, &[5, 6, 7]);
}

#[test]
fn q5_02_null_predicate() {
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE n(id INTEGER PRIMARY KEY, k INTEGER, flag INTEGER); \
         CREATE INDEX ix_n ON n(k) WHERE flag > 0; \
         CREATE INDEX ix_b ON n(k) WHERE flag; \
         INSERT INTO n VALUES (1, 10, NULL), (2, 20, 1), (3, 30, NULL), (4, 40, 1);",
    );
    // NULL is not true: NULL -> 2 enters, 1 -> NULL leaves, and a key change
    // under a NULL predicate stays out.
    lab.exec_both("UPDATE n SET flag = 2 WHERE id = 1");
    lab.exec_both("UPDATE n SET flag = NULL WHERE id = 2");
    lab.exec_both("UPDATE n SET k = 31 WHERE id = 3");
    // A member whose key becomes NULL stays a member under a NULL key.
    lab.exec_both("UPDATE n SET k = NULL WHERE id = 4");
    check_reads(&lab, "n", "ix_n", "flag > 0", &[10, 20, 30, 31]);
    check_reads(&lab, "n", "ix_b", "flag", &[10, 20, 30, 31]);
    for access in ["n NOT INDEXED", "n"] {
        lab.assert_same(
            &format!("SELECT id FROM {access} WHERE flag > 0 AND k IS NULL"),
            false,
        );
    }
    assert_members(&lab, "ix_n", "n", "flag > 0");
    assert_members(&lab, "ix_b", "n", "flag");
    assert_integrity_ok(&lab);
}

#[test]
fn q5_02_rowid_move() {
    let lab = Lab::new();
    lab.exec_both(T);
    lab.exec_both("INSERT INTO t VALUES (1, 100, 1), (2, 200, 0), (3, 300, 1)");
    // false -> true with a new rowid.
    lab.exec_both("UPDATE t SET id = 10, flag = 1 WHERE id = 2");
    // true -> false with a new rowid.
    lab.exec_both("UPDATE t SET id = 20, flag = 0 WHERE id = 3");
    // true -> true: only the rowid moves.
    lab.exec_both("UPDATE t SET id = 30 WHERE id = 1");
    // false -> false: the rowid and the key move.
    lab.exec_both("UPDATE t SET id = 40, k = 400 WHERE id = 20");
    check_t(&lab, &[100, 200, 300, 400]);
    lab.assert_rows(
        "SELECT id FROM t INDEXED BY ix_k WHERE flag = 1 AND k > 0 ORDER BY id",
        &[vec![int(10)], vec![int(30)]],
    );
}

#[test]
fn q5_02_replace_merge_and_cascade_paths() {
    let lab = Lab::new();
    lab.exec_both(T);
    lab.exec_both("INSERT INTO t VALUES (1, 100, 1), (2, 200, 0), (3, 300, 1), (4, 400, 0)");
    // REPLACE of an existing rowid rewrites the row in place.
    lab.exec_both("REPLACE INTO t VALUES (2, 200, 1)");
    lab.exec_both("INSERT OR REPLACE INTO t VALUES (3, 301, 0)");
    check_t(&lab, &[100, 200, 300, 301, 400]);
    // MERGE ... WHEN MATCHED THEN UPDATE (SQLite has no MERGE; it runs the
    // same UPDATEs).
    lab.exec_both("CREATE TABLE s(id INTEGER PRIMARY KEY, flag INTEGER); INSERT INTO s VALUES (1, 0), (4, 1);");
    lab.redline
        .execute("MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET flag = s.flag")
        .expect("merge");
    lab.sqlite
        .execute_batch("UPDATE t SET flag = 0 WHERE id = 1; UPDATE t SET flag = 1 WHERE id = 4;")
        .expect("sqlite merge");
    check_t(&lab, &[100, 200, 300, 301, 400]);
    // ON UPDATE CASCADE moves child rows into and out of a partial index.
    lab.exec_both(
        "PRAGMA foreign_keys = ON; \
         CREATE TABLE parent(id INTEGER PRIMARY KEY, code INTEGER UNIQUE); \
         CREATE TABLE child(id INTEGER PRIMARY KEY, \
             k INTEGER REFERENCES parent(code) ON UPDATE CASCADE, flag INTEGER); \
         CREATE INDEX child_k ON child(k) WHERE k > 10; \
         INSERT INTO parent VALUES (1, 5), (2, 20); \
         INSERT INTO child VALUES (1, 5, 0), (2, 20, 0), (3, 20, 1);",
    );
    lab.exec_both("UPDATE parent SET code = 15 WHERE id = 1");
    lab.exec_both("UPDATE parent SET code = 3 WHERE id = 2");
    lab.assert_rows(
        "SELECT id, k FROM child ORDER BY id",
        &[
            vec![int(1), int(15)],
            vec![int(2), int(3)],
            vec![int(3), int(3)],
        ],
    );
    check_reads(&lab, "child", "child_k", "k > 10", &[3, 5, 15, 20]);
    assert_members(&lab, "child_k", "child", "k > 10");
    assert_integrity_ok(&lab);
}

/// Tables without a rowid alias, so a stored record can be copied from one
/// rowid to another without changing the rowid it is read back under.
const SEEDED: &str = "CREATE TABLE p(id INT, k INTEGER, flag INTEGER); \
    CREATE INDEX p_k ON p(k) WHERE flag = 1; \
    INSERT INTO p VALUES (1, 100, 0), (2, 200, 1), (3, 300, 1), (4, 400, 0); \
    CREATE TABLE q(id INT, k INTEGER, flag INTEGER); \
    CREATE UNIQUE INDEX q_k ON q(k) WHERE flag = 1; \
    INSERT INTO q VALUES (1, 9, 0), (2, 9, 1), (3, 10, 1);";

/// Overwrite heap row `target` of `table` with the stored record of row
/// `source` and leave every index as it is: the state an UPDATE of
/// RedlineDB before this fix left when it changed a row's membership.
fn copy_heap_row(conn: &Connection, table: &str, source: u64, target: u64) {
    let engine = conn.engine_for_tests();
    let relation = engine
        .schema_snapshot()
        .tables
        .iter()
        .find(|def| def.name.as_ref() == table)
        .unwrap_or_else(|| panic!("no table {table}"))
        .relation_id;
    let mut tx = engine.begin(Isolation::ReadCommitted).expect("begin");
    let record = engine
        .get_for_relation(&mut tx, relation, RowId(source))
        .expect("read")
        .expect("source row");
    engine
        .update_for_relation(&mut tx, relation, RowId(target), record)
        .expect("overwrite");
    assert!(matches!(
        engine.commit(tx).expect("commit"),
        CommitOutcome::Committed(_)
    ));
}

/// Damage `p` as the old UPDATE did: its first row becomes a copy of the
/// second, a member without an index entry, and its third row a copy of the
/// fourth, out of the index but still holding its entry. Returns the rowids.
fn damage_p(conn: &Arc<Connection>) -> Vec<u64> {
    let p = rowids(conn, "p");
    copy_heap_row(conn, "p", p[1], p[0]);
    copy_heap_row(conn, "p", p[3], p[2]);
    p
}

/// Damage `q` as the old UPDATE did: its first row becomes a second member
/// with key 9, without an index entry. Returns the rowids.
fn damage_q(conn: &Arc<Connection>) -> Vec<u64> {
    let q = rowids(conn, "q");
    copy_heap_row(conn, "q", q[1], q[0]);
    q
}

/// SQLite's view of `p` after the damage. (SQLite cannot hold `q`'s second
/// member for key 9; the test deletes that row on both sides below.)
const P_DAMAGE_IN_SQLITE: &str = "UPDATE p SET id = 2, k = 200, flag = 1 WHERE id = 1; \
    UPDATE p SET id = 4, k = 400, flag = 0 WHERE id = 3;";

#[test]
fn q5_02_integrity_check_detects_seeded_corruption() {
    let lab = Lab::new();
    lab.exec_both(SEEDED);
    assert_integrity_ok(&lab);
    let p = damage_p(&lab.redline);
    let q = damage_q(&lab.redline);
    lab.sqlite
        .execute_batch(P_DAMAGE_IN_SQLITE)
        .expect("sqlite seed");
    let mut report = integrity_rows(&lab.redline);
    report.sort();
    let mut want = vec![
        format!(
            "index p_k has an entry for row {} that the row does not produce",
            p[2]
        ),
        "non-unique entry in UNIQUE index q_k".to_owned(),
        format!("row {} missing from index p_k", p[0]),
        format!("row {} missing from index q_k", q[0]),
        "wrong # of entries in index q_k".to_owned(),
    ];
    want.sort();
    assert_eq!(report, want);
    // Rebuilding the UNIQUE index over two members with one key fails.
    let err = lab.redline.execute("REINDEX q_k").unwrap_err().to_string();
    assert!(err.contains("UNIQUE constraint failed"), "{err}");
    lab.exec_both("REINDEX p_k");
    // The first row of `q` is (2, 9, 1) here and (1, 9, 0) in SQLite;
    // deleting it leaves the same rows on both sides.
    lab.redline
        .execute(&format!("DELETE FROM q WHERE rowid = {}", q[0]))
        .expect("redline delete");
    lab.sqlite
        .execute_batch("DELETE FROM q WHERE id = 1")
        .expect("sqlite delete");
    lab.exec_both("REINDEX q_k");
    assert_integrity_ok(&lab);
    check_reads(&lab, "p", "p_k", "flag = 1", &[100, 200, 300, 400]);
    check_reads(&lab, "q", "q_k", "flag = 1", &[9, 10]);
    assert_members(&lab, "p_k", "p", "flag = 1");
    assert_members(&lab, "q_k", "q", "flag = 1");
    // `quick_check` does not read index contents, as in SQLite.
    lab.assert_rows("PRAGMA quick_check", &[vec![text("ok")]]);
}

fn write_damaged_v4_database(path: &Path, setup: &str, damage: impl FnOnce(&Arc<Connection>)) {
    with_v4_index_format_for_tests(|| {
        let db = Database::create(path, DbOptions::default()).expect("create v4 database");
        let conn = db.connect();
        conn.execute(setup).expect("v4 setup");
        damage(&conn);
    });
}

#[test]
fn q5_02_damaged_v4_partial_index_is_rebuilt_at_open() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("v4.db");
    let setup = "CREATE TABLE p(id INT, k INTEGER, flag INTEGER); \
        CREATE INDEX p_k ON p(k) WHERE flag = 1; \
        INSERT INTO p VALUES (1, 100, 0), (2, 200, 1), (3, 300, 1), (4, 400, 0);";
    write_damaged_v4_database(&path, setup, |conn| {
        damage_p(conn);
    });
    let lab = Lab::open(&path);
    lab.sqlite
        .execute_batch(&format!("{setup} {P_DAMAGE_IN_SQLITE}"))
        .expect("sqlite replay");
    assert_integrity_ok(&lab);
    check_reads(&lab, "p", "p_k", "flag = 1", &[100, 200, 300, 400]);
    assert_members(&lab, "p_k", "p", "flag = 1");
}

#[test]
fn q5_02_v4_partial_unique_duplicates_fail_the_open() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("v4.db");
    let setup = "CREATE TABLE q(id INT, k INTEGER, flag INTEGER); \
        CREATE UNIQUE INDEX q_k ON q(k) WHERE flag = 1; \
        INSERT INTO q VALUES (1, 9, 0), (2, 9, 1), (3, 10, 1);";
    write_damaged_v4_database(&path, setup, |conn| {
        damage_q(conn);
    });
    let err = match Database::open(&path, DbOptions::default()) {
        Ok(_) => panic!("a v4 partial UNIQUE index with two members for one key opened"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("UNIQUE constraint failed"), "{err}");
    assert!(err.contains("q_k"), "{err}");
    assert!(err.contains("partial index"), "{err}");
    assert!(err.contains("the database was not changed"), "{err}");
}
