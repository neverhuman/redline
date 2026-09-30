//! An equality on an indexed column reads only the matching rows.
//!
//! The routed full scan pre-empted every index lookup except a unique one,
//! so `WHERE k = 5` on a non-unique index read the whole table while
//! EXPLAIN reported the index. A lookup on every key column of an index
//! returns its rows in rowid order, the scan's order, so it now runs.

mod perf_gate;

use std::sync::Arc;

use perf_gate::{assert_flat, open, work};
use redlinedb_sql::{Connection, SqlValue, Step};

/// Matching rows at every table size.
const MATCHES: i64 = 10;

/// `t(id, k, v)` with an index on `k`. `k = 5` for every `rows / 10`-th id,
/// so ten rows match at any size; every other row has its own `k`.
fn keyed_table(rows: i64) -> (tempfile::TempDir, Arc<Connection>) {
    let (dir, conn) = open();
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, k INTEGER, v TEXT)")
        .expect("create");
    conn.execute("CREATE INDEX t_k ON t(k)").expect("index");
    conn.execute("BEGIN").expect("begin");
    let mut insert = conn
        .prepare("INSERT INTO t(id, k, v) VALUES (?1, ?2, 'value')")
        .expect("prepare insert");
    let step = rows / MATCHES;
    for id in 1..=rows {
        insert.reset().expect("reset");
        insert.bind_i64(1, id).expect("bind id");
        let k = if id % step == 0 { 5 } else { rows + id };
        insert.bind_i64(2, k).expect("bind k");
        while insert.step().expect("insert") == Step::Row {}
    }
    drop(insert);
    conn.execute("COMMIT").expect("commit");
    (dir, conn)
}

fn ids(conn: &Arc<Connection>, sql: &str) -> Vec<i64> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let mut out = Vec::new();
    while stmt.step().expect("step") == Step::Row {
        let SqlValue::Integer(id) = stmt.column_value(0).expect("value").clone() else {
            panic!("expected an integer id");
        };
        out.push(id);
    }
    out
}

#[test]
fn literal_equality_on_a_non_unique_index_reads_only_its_rows() {
    // Each match is read twice today: once to check it is visible, once
    // to project it.
    assert_flat(
        "SELECT * FROM t WHERE k = 5",
        2 * MATCHES as u64,
        0,
        |rows| {
            let (_dir, conn) = keyed_table(rows);
            let (found, counted) = work(|| ids(&conn, "SELECT id, k, v FROM t WHERE k = 5"));
            let step = rows / MATCHES;
            let expected: Vec<i64> = (1..=MATCHES).map(|n| n * step).collect();
            assert_eq!(found, expected, "the rows, in rowid order");
            counted.relation_gets
        },
    );
}
