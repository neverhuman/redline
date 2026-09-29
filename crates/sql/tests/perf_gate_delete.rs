//! DELETE by rowid does work that does not grow with the table.
//!
//! Deleting a row used to rescan the whole table to lower the rowid
//! allocator, so a DELETE of k rows cost O(k·N). The allocator is now per
//! table and only a delete of the table's highest rowid needs the table's
//! new maximum.

mod perf_gate;

use perf_gate::{assert_flat, fill, open, run, work};

#[test]
fn deleting_a_row_by_rowid_reads_a_fixed_number_of_rows() {
    assert_flat("DELETE FROM t WHERE id = ?", 8, 2, |rows| {
        let (_dir, conn) = open();
        conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)")
            .expect("create");
        fill(&conn, "INSERT INTO t(id, v) VALUES (?, ?)", rows);
        let ((), counted) = work(|| run(&conn, "DELETE FROM t WHERE id = ?", &[rows / 2]));
        counted.relation_gets
    });
}

#[test]
fn deleting_rows_in_one_transaction_reads_a_fixed_number_per_row() {
    assert_flat(
        "20 DELETEs by rowid in one transaction",
        20 * 8,
        2 * 20,
        |rows| {
            let (_dir, conn) = open();
            conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)")
                .expect("create");
            fill(&conn, "INSERT INTO t(id, v) VALUES (?, ?)", rows);
            let ((), counted) = work(|| {
                conn.execute("BEGIN").expect("begin");
                for id in 1..=20 {
                    run(&conn, "DELETE FROM t WHERE id = ?", &[id]);
                }
                conn.execute("COMMIT").expect("commit");
            });
            counted.relation_gets
        },
    );
}
