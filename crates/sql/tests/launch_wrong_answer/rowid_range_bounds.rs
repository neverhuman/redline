//! A rowid range answers what a scan answers, and what SQLite answers.
//!
//! WHERE conjuncts that bound the rowid select the rows to read
//! (`exec/rowid_range.rs`). Every bound kind is compared with SQLite here,
//! in order and with storage classes: integer, REAL, numeric and other
//! text, NULL, blobs, negative bounds, the ends of the i64 range, empty and
//! reversed intervals, rowid aliases and the `rowid` name, reversed
//! operands, and a range beside an index. (RedlineDB refuses a negative
//! INTEGER PRIMARY KEY, so the table holds none.)

use redlinedb_sql::SqlValue;

use crate::lab::{Lab, int, real, text};

const SETUP: &str = "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT, k INTEGER); \
    CREATE INDEX t_k ON t(k); \
    INSERT INTO t VALUES (0, 'zero', 1), (1, 'one', 2), (2, 'two', 3), (5, 'five', 1), \
        (6, 'six', 2), (10, 'ten', 3), (100, 'hundred', 1), (9223372036854775807, 'max', 2); \
    CREATE TABLE plain(x TEXT); \
    INSERT INTO plain VALUES ('a'), ('b'), ('c'), ('d');";

#[test]
fn rowid_bounds_match_sqlite() {
    let lab = Lab::new();
    lab.exec_both(SETUP);
    for sql in [
        "SELECT id, v FROM t WHERE id BETWEEN 1 AND 6",
        "SELECT id, v FROM t WHERE id BETWEEN -5 AND 2",
        "SELECT id, v FROM t WHERE id BETWEEN 6 AND 1",
        "SELECT id, v FROM t WHERE id >= 2 AND id < 10",
        "SELECT id, v FROM t WHERE id > 2 AND id <= 10",
        "SELECT id, v FROM t WHERE id > 5.5",
        "SELECT id, v FROM t WHERE id < 5.5",
        "SELECT id, v FROM t WHERE id <= 5.0 AND id > 0.5",
        "SELECT id, v FROM t WHERE id = 5.0",
        "SELECT id, v FROM t WHERE id = 5.5",
        "SELECT id, v FROM t WHERE id > '5'",
        "SELECT id, v FROM t WHERE id < '6.5'",
        "SELECT id, v FROM t WHERE id > 'abc'",
        "SELECT id, v FROM t WHERE id < 'abc'",
        "SELECT id, v FROM t WHERE id > x'00'",
        "SELECT id, v FROM t WHERE id < x'00'",
        "SELECT id, v FROM t WHERE id > NULL",
        "SELECT id, v FROM t WHERE id BETWEEN NULL AND 5",
        "SELECT id, v FROM t WHERE id < 0",
        "SELECT id, v FROM t WHERE id >= -9223372036854775808 AND id < -1",
        "SELECT id, v FROM t WHERE id > 9223372036854775806",
        "SELECT id, v FROM t WHERE id < -9223372036854775807",
        "SELECT id, v FROM t WHERE id > 1e30",
        "SELECT id, v FROM t WHERE id < -1e30",
        "SELECT id, v FROM t WHERE id > -1e30 AND id < 1e30",
        "SELECT id, v FROM t WHERE 5 < id AND 100 >= id",
        "SELECT id, v FROM t WHERE rowid BETWEEN 1 AND 6",
        "SELECT id, v FROM t WHERE t.id BETWEEN 1 AND 6 AND v <> 'two'",
        "SELECT id, v FROM t WHERE id = 6 AND v = 'six'",
        "SELECT id, v FROM t WHERE id = 6 AND v = 'five'",
        "SELECT id, v FROM t WHERE id > 1 AND id > 5 AND id < 100",
        "SELECT id, v FROM t WHERE (id >= 1) AND (id <= 10) AND k = 2",
        "SELECT id, v FROM t WHERE id NOT BETWEEN 1 AND 6",
        "SELECT id, v FROM t WHERE id > 1 OR id < -1",
        "SELECT id, v FROM t NOT INDEXED WHERE id BETWEEN 1 AND 6",
        "SELECT sum(id), count(*) FROM t WHERE id BETWEEN -5 AND 10",
        "SELECT id FROM t WHERE id BETWEEN 1 AND 100 ORDER BY id DESC LIMIT 3",
        "SELECT id FROM t WHERE id BETWEEN 1 AND 100 LIMIT 2 OFFSET 1",
        "SELECT rowid, x FROM plain WHERE rowid BETWEEN 2 AND 3",
        "SELECT rowid, x FROM plain WHERE _rowid_ > 2",
    ] {
        lab.assert_same(sql, true);
    }
    // An index range beside a rowid range takes the index, as before, and
    // returns its rows in index order; SQLite takes the rowid range here.
    // Without ORDER BY either order is correct, so compare the rows only.
    lab.assert_same(
        "SELECT id, v FROM t WHERE id BETWEEN 0 AND 10 AND k BETWEEN 2 AND 3",
        false,
    );
    for (low, high) in [
        (int(1), int(6)),
        (real(1.5), real(6.5)),
        (text("2"), text("10")),
        (text("x"), int(6)),
        (int(-9223372036854775808), int(-1)),
        (int(9223372036854775807), int(9223372036854775807)),
    ] {
        lab.assert_same_bound(
            "SELECT id, v FROM t WHERE id BETWEEN ? AND ?",
            &[low.clone(), high.clone()],
            true,
        );
        lab.assert_same_bound(
            "SELECT id, v FROM t WHERE id >= ?1 AND id < ?2",
            &[low, high],
            true,
        );
    }
}

#[test]
fn rowid_ranges_over_changed_rows_match_sqlite() {
    let lab = Lab::new();
    lab.exec_both(SETUP);
    // A deleted row, a row grown past its slot (a new version elsewhere),
    // and a key moved into and out of the range.
    lab.exec_both(
        "DELETE FROM t WHERE id = 6; \
         UPDATE t SET v = v || 'grown past its slot, grown past its slot' WHERE id = 5; \
         UPDATE t SET id = 7 WHERE id = 2; \
         UPDATE t SET id = 1000 WHERE id = 10;",
    );
    for sql in [
        "SELECT id, v FROM t WHERE id BETWEEN 5 AND 5",
        "SELECT id, v FROM t WHERE id >= 5 AND id <= 5",
        "SELECT id, v FROM t WHERE id BETWEEN 6 AND 6",
        "SELECT id, v FROM t WHERE id BETWEEN 0 AND 10",
        "SELECT id, v FROM t WHERE id BETWEEN 1 AND 1000",
        "SELECT id, v FROM t WHERE id > 9223372036854775807",
        "SELECT id, v FROM t WHERE id < -9223372036854775808",
        "SELECT id, v FROM t WHERE id >= 9223372036854775807",
    ] {
        lab.assert_same(sql, true);
    }
    // Inside a transaction, its own uncommitted changes count.
    lab.exec_both(
        "BEGIN; INSERT INTO t VALUES (3, 'three', 1); DELETE FROM t WHERE id = 7; \
         UPDATE t SET v = 'changed' WHERE id = 1;",
    );
    lab.assert_same("SELECT id, v FROM t WHERE id BETWEEN 1 AND 7", true);
    lab.exec_both("ROLLBACK;");
    lab.assert_same("SELECT id, v FROM t WHERE id BETWEEN 1 AND 7", true);
}

#[test]
fn rowid_ranges_on_other_table_kinds_match_sqlite() {
    let lab = Lab::new();
    lab.exec_both(SETUP);
    lab.exec_both(
        "CREATE TEMP TABLE tt(id INTEGER PRIMARY KEY, v TEXT); \
         INSERT INTO tt VALUES (1, 'a'), (2, 'b'), (3, 'c'), (9, 'i'); \
         CREATE VIEW tv AS SELECT id, v FROM t WHERE k <> 3; \
         CREATE TABLE w(k INTEGER PRIMARY KEY, v TEXT) WITHOUT ROWID; \
         INSERT INTO w VALUES (1, 'a'), (2, 'b'), (5, 'e'), (9, 'i'); \
         CREATE TABLE s(rowid TEXT, x INTEGER); \
         INSERT INTO s VALUES ('9', 1), ('1', 2), ('5', 3);",
    );
    for sql in [
        "SELECT id, v FROM tt WHERE id BETWEEN 2 AND 9",
        "SELECT id, v FROM tv WHERE id BETWEEN 1 AND 100",
        "SELECT k, v FROM w WHERE k BETWEEN 2 AND 5",
        "SELECT rowid, x FROM s WHERE rowid BETWEEN 1 AND 5",
        "SELECT x FROM s WHERE rowid > 2",
    ] {
        lab.assert_same(sql, true);
    }
}

#[test]
fn a_rowid_range_reads_its_snapshot() {
    // Another connection's commits after the snapshot do not show; the
    // `NOT INDEXED` scan in the same transaction is the reference.
    let lab = Lab::new();
    lab.exec_both(SETUP);
    let reader = &lab.redline;
    let writer = lab.database.connect();
    reader.execute("BEGIN").expect("begin");
    let range = "SELECT id, v FROM t WHERE id BETWEEN 0 AND 100";
    let scan = "SELECT id, v FROM t NOT INDEXED WHERE id BETWEEN 0 AND 100";
    let before = rows(reader, range);
    writer
        .execute("INSERT INTO t VALUES (50, 'fifty', 1)")
        .expect("insert");
    writer
        .execute("DELETE FROM t WHERE id = 5")
        .expect("delete");
    writer
        .execute("UPDATE t SET v = 'later' WHERE id = 1")
        .expect("update");
    assert_eq!(rows(reader, range), before, "range inside the snapshot");
    assert_eq!(rows(reader, scan), before, "scan inside the snapshot");
    reader.execute("COMMIT").expect("commit");
    assert_eq!(rows(reader, range), rows(reader, scan), "range after it");
}

fn rows(conn: &std::sync::Arc<redlinedb_sql::Connection>, sql: &str) -> Vec<Vec<SqlValue>> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let mut out = Vec::new();
    while stmt.step().expect("step") == redlinedb_sql::Step::Row {
        out.push(
            (0..stmt.column_count())
                .map(|i| stmt.column_value(i).expect("value").clone())
                .collect(),
        );
    }
    out
}
