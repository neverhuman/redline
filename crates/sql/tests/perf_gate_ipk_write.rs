//! Writes to a table with an INTEGER PRIMARY KEY check the key with one
//! read, not a scan of the table.
//!
//! Every INSERT, and every row an UPDATE wrote, used to collect all of the
//! table's rowids to test whether the new key was taken, so loading N rows
//! cost O(N²). The check now reads the one rowid the key names.

mod perf_gate;

use perf_gate::{assert_flat, fill, open, run, work};

const CREATE: &str = "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)";
const FILL: &str = "INSERT INTO t(id, v) VALUES (?, ?)";

#[test]
fn inserting_an_explicit_key_reads_a_fixed_number_of_rows() {
    assert_flat("INSERT INTO t(id, v) VALUES (?, 'x')", 4, 1, |rows| {
        let (_dir, conn) = open();
        conn.execute(CREATE).expect("create");
        fill(&conn, FILL, rows);
        let ((), counted) = work(|| {
            run(&conn, "INSERT INTO t(id, v) VALUES (?, 'x')", &[rows + 1]);
        });
        counted.relation_gets
    });
}

#[test]
fn inserting_a_null_key_reads_a_fixed_number_of_rows() {
    assert_flat("INSERT INTO t(v) VALUES ('x')", 4, 1, |rows| {
        let (_dir, conn) = open();
        conn.execute(CREATE).expect("create");
        fill(&conn, FILL, rows);
        let ((), counted) = work(|| run(&conn, "INSERT INTO t(v) VALUES ('x')", &[]));
        counted.relation_gets
    });
}

#[test]
fn an_autoincrement_insert_reads_a_fixed_number_of_rows() {
    assert_flat(
        "INSERT INTO t(v) VALUES ('x') with AUTOINCREMENT",
        4,
        1,
        |rows| {
            let (_dir, conn) = open();
            conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT, v TEXT)")
                .expect("create");
            fill(&conn, FILL, rows);
            let ((), counted) = work(|| run(&conn, "INSERT INTO t(v) VALUES ('x')", &[]));
            counted.relation_gets
        },
    );
}

#[test]
fn updating_a_non_key_column_reads_a_fixed_number_of_rows() {
    assert_flat("UPDATE t SET v = 'y' WHERE id = ?", 6, 1, |rows| {
        let (_dir, conn) = open();
        conn.execute(CREATE).expect("create");
        fill(&conn, FILL, rows);
        let ((), counted) = work(|| run(&conn, "UPDATE t SET v = 'y' WHERE id = ?", &[rows / 2]));
        counted.relation_gets
    });
}
