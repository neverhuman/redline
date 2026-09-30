//! An equality on every key column of an index returns its rows in the
//! order SQLite does, without ORDER BY.
//!
//! Such a lookup no longer yields to the routed full scan
//! (`exec::select_route_gate`), so its order is now the index's: rowid
//! order within one key. Rows here are inserted out of rowid order, one
//! moves to another heap page, and some are deleted, so neither insertion
//! order nor heap order matches rowid order.

use crate::lab::{Lab, int};

const SETUP: &str = "CREATE TABLE t(id INTEGER PRIMARY KEY, k INTEGER, j INTEGER, v TEXT); \
    CREATE INDEX t_k ON t(k); \
    CREATE INDEX t_kj ON t(k, j); \
    INSERT INTO t VALUES (40, 5, 1, 'a'), (3, 5, 1, 'b'), (17, 6, 2, 'c'), (9, 5, 2, 'd'), \
        (1, 5, 1, 'e'), (25, 6, 1, 'f'), (12, 5, 1, 'g'), (30, NULL, 1, 'h'), (7, 5, 2, 'i'); \
    UPDATE t SET v = v || 'moved and grown past its slot' WHERE id = 3; \
    DELETE FROM t WHERE id = 9;";

#[test]
fn index_points_return_rows_in_sqlite_order() {
    let lab = Lab::new();
    lab.exec_both(SETUP);
    for sql in [
        "SELECT id, v FROM t WHERE k = 5",
        "SELECT id, v FROM t WHERE k = 5 AND v <> 'e'",
        "SELECT v FROM t WHERE k = 5 AND j = 1",
        "SELECT id FROM t WHERE k = 6",
        "SELECT id FROM t WHERE k IS NULL",
        "SELECT id, v FROM t INDEXED BY t_k WHERE k = 5",
    ] {
        lab.assert_same(sql, true);
    }
    lab.assert_same_bound("SELECT id, v FROM t WHERE k = ?", &[int(5)], true);
}
