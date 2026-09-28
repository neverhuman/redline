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
