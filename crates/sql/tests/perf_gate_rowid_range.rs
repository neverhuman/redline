//! A range on the rowid reads only the rows in the range.
//!
//! `WHERE id BETWEEN ? AND ?` on an INTEGER PRIMARY KEY had no access path
//! when no index applied: the query read every row of the table and
//! evaluated the range on each. The rowid bounds now select the rows to
//! read.

mod perf_gate;

use perf_gate::{assert_flat, fill, open, work};
use redlinedb_sql::{SqlValue, Step};

/// Rows in each range.
const SPAN: i64 = 100;

fn integers(
    conn: &std::sync::Arc<redlinedb_sql::Connection>,
    sql: &str,
    params: &[i64],
) -> Vec<i64> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    for (index, value) in params.iter().enumerate() {
        stmt.bind_i64(index + 1, *value).expect("bind");
    }
    let mut out = Vec::new();
    while stmt.step().expect("step") == Step::Row {
        match stmt.column_value(0).expect("value").clone() {
            SqlValue::Integer(value) => out.push(value),
            other => panic!("expected an integer, got {other:?}"),
        }
    }
    out
}

fn table(rows: i64) -> (tempfile::TempDir, std::sync::Arc<redlinedb_sql::Connection>) {
    let (dir, conn) = open();
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT, n INTEGER)")
        .expect("create");
    fill(&conn, "INSERT INTO t(id, v) VALUES (?, ?)", rows);
    conn.execute("UPDATE t SET n = id % 7").expect("fill n");
    (dir, conn)
}

/// The rows one query reads, checking it returns `expected`.
fn reads(rows: i64, sql: &str, params: &[i64], expected: impl Fn(i64) -> Vec<i64>) -> u64 {
    let (_dir, conn) = table(rows);
    let (found, counted) = work(|| integers(&conn, sql, params));
    assert_eq!(found, expected(rows), "{sql}");
    counted.relation_gets
}

#[test]
fn rowid_between_reads_only_the_range() {
    assert_flat(
        "SELECT id FROM t WHERE id BETWEEN ? AND ?",
        SPAN as u64,
        0,
        |rows| {
            let low = rows / 2;
            reads(
                rows,
                "SELECT id FROM t WHERE id BETWEEN ? AND ?",
                &[low, low + SPAN - 1],
                |rows| (rows / 2..rows / 2 + SPAN).collect(),
            )
        },
    );
}

#[test]
fn half_open_rowid_range_reads_only_the_range() {
    assert_flat(
        "SELECT id, v FROM t WHERE id >= ? AND id < ?",
        SPAN as u64,
        0,
        |rows| {
            let low = rows / 3;
            reads(
                rows,
                "SELECT id, v FROM t WHERE id >= ? AND id < ?",
                &[low, low + SPAN],
                |rows| (rows / 3..rows / 3 + SPAN).collect(),
            )
        },
    );
}

#[test]
fn aggregate_over_a_rowid_range_reads_only_the_range() {
    assert_flat(
        "SELECT sum(n) FROM t WHERE id BETWEEN ? AND ?",
        SPAN as u64,
        0,
        |rows| {
            let low = rows / 4;
            reads(
                rows,
                "SELECT sum(n) FROM t WHERE id BETWEEN ? AND ?",
                &[low, low + SPAN - 1],
                |rows| vec![(rows / 4..rows / 4 + SPAN).map(|id| id % 7).sum()],
            )
        },
    );
}

#[test]
fn literal_rowid_range_reads_only_the_range() {
    // Literal bounds used to take the routed full scan.
    assert_flat(
        "SELECT id FROM t WHERE id > 100 AND id <= 199",
        SPAN as u64 - 1,
        0,
        |rows| {
            reads(
                rows,
                "SELECT id FROM t WHERE id > 100 AND id <= 199",
                &[],
                |_| (101..=199).collect(),
            )
        },
    );
}

#[test]
fn rowid_equality_beside_another_conjunct_reads_one_row() {
    assert_flat("SELECT id FROM t WHERE id = ? AND n = ?", 1, 0, |rows| {
        let id = rows - 3;
        reads(
            rows,
            "SELECT id FROM t WHERE id = ? AND n = ?",
            &[id, id % 7],
            |rows| vec![rows - 3],
        )
    });
}
