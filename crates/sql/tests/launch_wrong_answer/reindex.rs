//! IDX-EPOCH: `REINDEX` rebuilds indexes.
//!
//! `REINDEX` was a no-op, and the parser accepted only the bare statement:
//! `REINDEX t`, `REINDEX i`, `REINDEX main.t` and `REINDEX nocase` all failed
//! to prepare. It now rebuilds through the same mechanism as the open-time
//! index-format upgrade, and resolves a name the way SQLite's
//! `sqlite3Reindex` does: an unqualified collation name first, then a table,
//! then an index, else `unable to identify the object to be reindexed`.

use std::collections::BTreeMap;
use std::sync::Arc;

use redlinedb_sql::Connection;

use crate::lab::Lab;

const SETUP: &str = "CREATE TABLE t(x, y); \
    CREATE INDEX t_x ON t(x); \
    CREATE INDEX t_y ON t(y COLLATE NOCASE); \
    CREATE UNIQUE INDEX t_xy ON t(x, y); \
    CREATE INDEX t_expr ON t(x * 2); \
    CREATE INDEX t_part ON t(y) WHERE x > 1; \
    CREATE TABLE s(z); CREATE INDEX s_z ON s(z); \
    INSERT INTO t VALUES (1,'a'),(2.5,'B'),(3,'c'),(2,'D'),(2.0,'e'),(1.5,'f'); \
    INSERT INTO s VALUES (1),(1.5),(2.0),(3);";

/// Queries that read every index of `t` and `s`.
const CHECKS: &[(&str, bool)] = &[
    (
        "SELECT x FROM t INDEXED BY t_x WHERE x > 1.5 ORDER BY x, y",
        true,
    ),
    ("SELECT y FROM t INDEXED BY t_x WHERE x = 2", false),
    (
        "SELECT y FROM t INDEXED BY t_xy WHERE x = 2 AND y > 'A'",
        false,
    ),
    ("SELECT y FROM t INDEXED BY t_expr WHERE x * 2 = 4", false),
    (
        "SELECT y FROM t INDEXED BY t_part WHERE x > 1 AND y > ''",
        false,
    ),
    (
        "SELECT z FROM s INDEXED BY s_z WHERE z < 2.5 ORDER BY z",
        true,
    ),
    ("PRAGMA integrity_check", true),
];

/// Index name -> B-tree meta page, read from the live catalog.
fn meta_pages(conn: &Arc<Connection>) -> BTreeMap<String, u64> {
    conn.engine_for_tests()
        .schema_snapshot()
        .indexes
        .iter()
        .map(|index| {
            let meta = index.meta_page_id.expect("physical index").0;
            (index.name.to_string(), meta)
        })
        .collect()
}

fn rebuilt(before: &BTreeMap<String, u64>, after: &BTreeMap<String, u64>) -> Vec<String> {
    before
        .iter()
        .filter(|(name, meta)| after.get(*name) != Some(meta))
        .map(|(name, _)| name.clone())
        .collect()
}

fn check(lab: &Lab) {
    for (sql, ordered) in CHECKS {
        lab.assert_same(sql, *ordered);
    }
}

#[test]
fn reindex_rebuilds_the_named_indexes() {
    let lab = Lab::new();
    lab.exec_both(SETUP);
    let cases: &[(&str, &[&str])] = &[
        (
            "REINDEX",
            &["s_z", "t_expr", "t_part", "t_x", "t_xy", "t_y"],
        ),
        ("REINDEX t", &["t_expr", "t_part", "t_x", "t_xy", "t_y"]),
        ("REINDEX t_x", &["t_x"]),
        ("REINDEX main.s", &["s_z"]),
        ("REINDEX main.t_xy", &["t_xy"]),
        ("REINDEX \"t_expr\"", &["t_expr"]),
        ("REINDEX [t_part]", &["t_part"]),
        ("REINDEX `S_Z`;", &["s_z"]),
        // Collation form: every index with a key column using it.
        ("REINDEX nocase", &["t_y"]),
        ("REINDEX BINARY", &["s_z", "t_part", "t_x", "t_xy"]),
        ("REINDEX rtrim", &[]),
    ];
    for (sql, want) in cases {
        let before = meta_pages(&lab.redline);
        lab.step_both(sql);
        let after = meta_pages(&lab.redline);
        assert_eq!(
            rebuilt(&before, &after),
            *want,
            "indexes rebuilt by `{sql}`"
        );
        check(&lab);
    }
}

#[test]
fn reindex_rejects_what_sqlite_rejects() {
    let lab = Lab::new();
    lab.exec_both(SETUP);
    let before = meta_pages(&lab.redline);
    for sql in [
        "REINDEX nosuch",
        "REINDEX main.nocase",
        "REINDEX temp.t",
        "REINDEX nosuch.t",
        "REINDEX t t",
        "REINDEX main.",
    ] {
        lab.step_both(sql);
    }
    assert_eq!(meta_pages(&lab.redline), before);
    check(&lab);
}

#[test]
fn reindex_follows_its_transaction() {
    let lab = Lab::new();
    lab.exec_both(SETUP);
    let before = meta_pages(&lab.redline);
    lab.step_both("BEGIN; REINDEX t; INSERT INTO t VALUES (4, 'g'); ROLLBACK;");
    assert_eq!(meta_pages(&lab.redline), before, "a rolled-back REINDEX");
    check(&lab);

    lab.step_both(
        "BEGIN; INSERT INTO t VALUES (4, 'g'); REINDEX t; INSERT INTO t VALUES (4.5, 'h'); COMMIT;",
    );
    let after = meta_pages(&lab.redline);
    assert_eq!(
        rebuilt(&before, &after),
        ["t_expr", "t_part", "t_x", "t_xy", "t_y"]
    );
    check(&lab);
    lab.assert_same(
        "SELECT y FROM t INDEXED BY t_x WHERE x >= 4 ORDER BY x",
        true,
    );
}

#[test]
fn reindex_reaches_temp_tables() {
    let lab = Lab::new();
    lab.exec_both(SETUP);
    lab.exec_both(
        "CREATE TEMP TABLE tt(z); CREATE INDEX tt_z ON tt(z); INSERT INTO tt VALUES (1),(1.5),(2.0);",
    );
    for sql in [
        "REINDEX temp.tt",
        "REINDEX temp.tt_z",
        "REINDEX main.tt",
        "REINDEX tt",
    ] {
        lab.step_both(sql);
        lab.assert_same(
            "SELECT z FROM tt INDEXED BY tt_z WHERE z > 0 ORDER BY z",
            true,
        );
    }
}

/// Reads between REINDEX and COMMIT go through the rebuilt B-tree, the one
/// the same transaction's writes maintain. They read the old B-tree, so rows
/// written after the REINDEX were missing from indexed reads until COMMIT.
#[test]
fn reindex_reads_its_own_writes_before_commit() {
    let lab = Lab::new();
    lab.exec_both(SETUP);
    let reads = [
        ("SELECT y FROM t INDEXED BY t_x WHERE x = 5", false),
        ("SELECT y FROM t INDEXED BY t_x WHERE x = 7", false),
        ("SELECT y FROM t INDEXED BY t_x WHERE x = 3", false),
        (
            "SELECT x FROM t INDEXED BY t_x WHERE x > 0 ORDER BY x, y",
            true,
        ),
        ("SELECT count(*) FROM t INDEXED BY t_x WHERE x > 0", true),
        (
            "SELECT y FROM t INDEXED BY t_xy WHERE x = 7 AND y > ''",
            false,
        ),
        ("SELECT y FROM t INDEXED BY t_expr WHERE x * 2 = 14", false),
        (
            "SELECT y FROM t INDEXED BY t_part WHERE x > 1 AND y > ''",
            false,
        ),
        ("SELECT y FROM t INDEXED BY t_y WHERE y = 'NEW'", false),
    ];
    lab.exec_both("BEGIN");
    lab.exec_both("REINDEX t");
    lab.exec_both("INSERT INTO t VALUES (5, 'new')");
    lab.exec_both("UPDATE t SET x = 7 WHERE y = 'a'");
    lab.exec_both("DELETE FROM t WHERE y = 'c'");
    for (sql, ordered) in reads {
        lab.assert_same(sql, ordered);
    }
    lab.exec_both("COMMIT");
    for (sql, ordered) in reads {
        lab.assert_same(sql, ordered);
    }
    check(&lab);
}

/// REINDEX swaps the index's B-tree for every connection at COMMIT, so it
/// commits only while its transaction is the one open and nothing committed
/// since its rebuild read the table. A snapshot reader that began earlier
/// would otherwise read the new B-tree and see none of its entries, and a
/// writer's changes would land in the B-tree the swap retires. The REINDEX
/// then fails as busy (SQLite's REINDEX takes an exclusive lock) and
/// changes nothing.
mod concurrent {
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
}
