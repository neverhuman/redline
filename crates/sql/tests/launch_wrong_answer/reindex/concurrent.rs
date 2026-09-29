//! REINDEX swaps the index's B-tree for every connection at COMMIT, so it
//! commits only while its transaction is the one open and nothing committed
//! since its rebuild read the table. A snapshot reader that began earlier
//! would otherwise read the new B-tree and see none of its entries, and a
//! writer's changes would land in the B-tree the swap retires. The REINDEX
//! then fails as busy (SQLite's REINDEX takes an exclusive lock) and
//! changes nothing.

use std::sync::Arc;
use std::time::Duration;

use redlinedb_sql::{Connection, SqlValue, Step};

use super::{meta_pages, rebuilt};
use crate::lab::{Lab, int};

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

fn integrity_ok(conn: &Arc<Connection>) {
    assert_eq!(
        rows(conn, "PRAGMA integrity_check"),
        vec![vec![SqlValue::Text("ok".into())]]
    );
}

fn setup() -> Lab {
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE t(x, y); CREATE INDEX t_x ON t(x); \
         INSERT INTO t VALUES (1, 'a'), (2, 'b'), (3, 'c');",
    );
    lab.redline.set_busy_timeout(Duration::from_millis(100));
    lab
}

const PROBE: &str = "SELECT y FROM t INDEXED BY t_x WHERE x = 2";

#[test]
fn a_snapshot_reader_keeps_its_index() {
    let lab = setup();
    let reader = lab.database.connect();
    reader.execute("BEGIN").expect("begin");
    assert_eq!(rows(&reader, PROBE).len(), 1);
    let before = meta_pages(&lab.redline);
    let err = lab
        .redline
        .execute("REINDEX t")
        .expect_err("a reader is open");
    assert!(err.to_string().contains("lock timeout"), "{err}");
    assert_eq!(
        meta_pages(&lab.redline),
        before,
        "the busy REINDEX changed nothing"
    );
    assert_eq!(rows(&reader, PROBE).len(), 1);
    reader.execute("COMMIT").expect("commit");
    lab.redline.execute("REINDEX t").expect("alone now");
    assert_eq!(rebuilt(&before, &meta_pages(&lab.redline)), ["t_x"]);
    assert_eq!(rows(&reader, PROBE).len(), 1);
    integrity_ok(&lab.redline);
}

#[test]
fn an_open_writer_keeps_its_rows() {
    let lab = setup();
    let writer = lab.database.connect();
    writer.execute("BEGIN").expect("begin");
    writer
        .execute("INSERT INTO t VALUES (9, 'w'); UPDATE t SET x = 7 WHERE y = 'a';")
        .expect("write");
    lab.redline
        .execute("REINDEX t")
        .expect_err("a writer is open");
    writer.execute("COMMIT").expect("commit");
    integrity_ok(&lab.redline);
    lab.redline.execute("REINDEX t").expect("alone now");
    integrity_ok(&lab.redline);
    for (x, want) in [(9, "w"), (7, "a"), (1, "")] {
        let got = rows(
            &lab.redline,
            &format!("SELECT y FROM t INDEXED BY t_x WHERE x = {x}"),
        );
        let want: Vec<Vec<SqlValue>> = if want.is_empty() {
            Vec::new()
        } else {
            vec![vec![SqlValue::Text(want.into())]]
        };
        assert_eq!(got, want, "x = {x}");
    }
}

#[test]
fn a_commit_after_the_rebuild_fails_the_reindex() {
    let lab = setup();
    let other = lab.database.connect();
    let before = meta_pages(&lab.redline);
    lab.redline.execute("BEGIN").expect("begin");
    lab.redline.execute("REINDEX t").expect("rebuild");
    // Another connection commits a row the rebuild did not read.
    other
        .execute("INSERT INTO t VALUES (4, 'd')")
        .expect("autocommit insert");
    let err = lab.redline.execute("COMMIT").expect_err("stale rebuild");
    assert!(err.to_string().contains("lock timeout"), "{err}");
    assert_eq!(meta_pages(&lab.redline), before);
    integrity_ok(&lab.redline);
    assert_eq!(
        rows(&lab.redline, "SELECT y FROM t INDEXED BY t_x WHERE x = 4"),
        vec![vec![SqlValue::Text("d".into())]]
    );
    // A reader that began after the rebuild blocks its COMMIT too.
    lab.redline.execute("BEGIN").expect("begin");
    lab.redline.execute("REINDEX t").expect("rebuild");
    other.execute("BEGIN").expect("begin");
    assert_eq!(rows(&other, "SELECT count(*) FROM t"), vec![vec![int(4)]]);
    lab.redline.execute("COMMIT").expect_err("a reader is open");
    other.execute("COMMIT").expect("commit");
    assert_eq!(meta_pages(&lab.redline), before);
    lab.redline.execute("REINDEX").expect("alone now");
    integrity_ok(&lab.redline);
}
