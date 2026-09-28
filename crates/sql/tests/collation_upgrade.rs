//! Q5-10 step 4: an existing database's indexes on columns declared NOCASE
//! or RTRIM get those collations at open.
//!
//! RedlineDB 4.x built such an index with BINARY keys unless the index named
//! a collation itself. `with_v4_key_collations_for_tests` makes this thread
//! create index keys that way, so a database written inside it is what an
//! upgrade finds. Opening it must give every such index its column's
//! collation, in the same transaction as the index-format upgrade, and must
//! fail, naming the index and changing nothing, when two rows then share a
//! key of a UNIQUE index.

use std::path::Path;
use std::sync::Arc;

use redlinedb_kernel::catalog::collation::with_v4_key_collations_for_tests;
use redlinedb_kernel::engine::Engine;
use redlinedb_sql::{Connection, Database, DbOptions, Step};

fn rows(conn: &Arc<Connection>, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let mut out = Vec::new();
    while matches!(stmt.step().expect("step"), Step::Row) {
        let cols = (0..stmt.column_count()).map(|i| {
            stmt.column_text(i)
                .map(|s| s.to_owned())
                .unwrap_or_else(|_| stmt.column_i64(i).unwrap_or(0).to_string())
        });
        out.push(cols.collect::<Vec<_>>().join("|"));
    }
    out
}

fn write_v4_database(path: &Path, statements: &[&str]) {
    with_v4_key_collations_for_tests(|| {
        let db = Database::create(path, DbOptions::default()).expect("create database");
        let conn = db.connect();
        for sql in statements {
            conn.execute(sql)
                .unwrap_or_else(|err| panic!("{sql}: {err}"));
        }
    });
}

/// `(index, key collations)` as the catalog holds them, read through the
/// kernel alone so nothing is upgraded.
fn key_collations(path: &Path) -> Vec<(String, Vec<Option<String>>)> {
    let engine = Engine::open(path, DbOptions::default().engine).expect("kernel open");
    let mut out: Vec<(String, Vec<Option<String>>)> = engine
        .schema_snapshot()
        .indexes
        .iter()
        .map(|index| {
            (
                index.name.to_string(),
                index
                    .keys
                    .iter()
                    .map(|key| key.collation.as_deref().map(str::to_owned))
                    .collect(),
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn upgrade_rebuild_detects_duplicates() {
    for (setup, index) in [
        (
            // A UNIQUE index with a B-tree.
            &[
                "CREATE TABLE t(x TEXT COLLATE NOCASE)",
                "CREATE UNIQUE INDEX t_x ON t(x)",
                "INSERT INTO t VALUES ('x')",
                "INSERT INTO t VALUES ('X')",
            ][..],
            "t_x",
        ),
        (
            // A column UNIQUE constraint, checked without a B-tree.
            &[
                "CREATE TABLE r(x TEXT COLLATE RTRIM UNIQUE)",
                "INSERT INTO r VALUES ('x')",
                "INSERT INTO r VALUES ('x ')",
            ][..],
            "sqlite_autoindex_r_1",
        ),
    ] {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("v4.db");
        write_v4_database(&path, setup);
        let before = key_collations(&path);
        assert!(
            before
                .iter()
                .any(|(name, keys)| name == index && keys == &[None]),
            "{index} should start with BINARY keys: {before:?}"
        );
        for attempt in 0..2 {
            let err = match Database::open(&path, DbOptions::default()) {
                Ok(_) => panic!("{index}: attempt {attempt}: the open kept two equal keys"),
                Err(err) => err.to_string(),
            };
            assert!(err.contains("UNIQUE"), "{index}: {err}");
            assert!(
                err.contains(index),
                "{index}: the error names another index: {err}"
            );
            assert!(err.contains("NOCASE or RTRIM"), "{index}: {err}");
            // Nothing was committed: the index still has its BINARY keys.
            assert_eq!(key_collations(&path), before, "{index}: attempt {attempt}");
        }
    }
}

#[test]
fn upgrade_gives_existing_indexes_their_declared_collation() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("v4.db");
    write_v4_database(
        &path,
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, x TEXT COLLATE NOCASE)",
            "CREATE INDEX t_x ON t(x)",
            "CREATE INDEX t_x_bin ON t(x COLLATE BINARY)",
            "CREATE TABLE u(x TEXT COLLATE NOCASE UNIQUE)",
            "INSERT INTO t(x) VALUES ('b'), ('A'), ('a'), ('B')",
            "INSERT INTO u VALUES ('a')",
        ],
    );
    let db = Database::open(&path, DbOptions::default()).expect("upgrade open");
    let conn = db.connect();
    // The NOCASE comparison can now use t_x; before, only a scan answered it.
    let plan = rows(&conn, "EXPLAIN QUERY PLAN SELECT x FROM t WHERE x = 'A'").join("\n");
    assert!(plan.contains("USING INDEX t_x"), "{plan}");
    assert_eq!(
        rows(&conn, "SELECT x FROM t WHERE x = 'A' ORDER BY id"),
        ["A", "a"]
    );
    assert_eq!(
        rows(&conn, "SELECT count(*) FROM t INDEXED BY t_x WHERE x > 'a'"),
        ["2"]
    );
    // The UNIQUE constraint now compares NOCASE.
    let err = conn
        .execute("INSERT INTO u VALUES ('A')")
        .expect_err("'A' after 'a' on a NOCASE UNIQUE column");
    assert!(err.to_string().contains("UNIQUE"), "{err}");
    drop(conn);
    drop(db);
    let after = key_collations(&path);
    let keys = |name: &str| {
        after
            .iter()
            .find(|(index, _)| index == name)
            .unwrap_or_else(|| panic!("{name} in {after:?}"))
            .1
            .clone()
    };
    assert_eq!(keys("t_x"), [Some("NOCASE".to_owned())]);
    // A key that names BINARY keeps it.
    assert_eq!(keys("t_x_bin"), [Some("BINARY".to_owned())]);
    assert_eq!(keys("sqlite_autoindex_u_1"), [Some("NOCASE".to_owned())]);
    // A second open has nothing left to upgrade.
    let engine = Engine::open(&path, DbOptions::default().engine).expect("kernel open");
    assert!(engine.indexes_needing_inherited_collation().is_empty());
    assert!(
        engine
            .indexes_needing_rebuild()
            .expect("rebuild list")
            .is_empty()
    );
}
