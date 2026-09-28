//! IDX-EPOCH: the index-format epoch, the open-time rebuild, and the
//! refusal in both directions between RedlineDB 4.x and 5.
//!
//! The epoch is the `u16` at offset 4 of every B-tree page's special area
//! (`redlinedb_kernel::index::INDEX_VERSION`): 2 in 4.x, 3 now. A database
//! written by 4.x is created here with
//! `with_v4_index_format_for_tests`, which makes this thread write index keys
//! and B-tree pages exactly as 4.1.0 did (INTEGER and REAL under separate
//! key tags). Opening it must rebuild every index from the heap, in one
//! transaction, before any query can read one.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use redlinedb_kernel::catalog::with_v4_index_format_for_tests;
use redlinedb_kernel::engine::Engine;
use redlinedb_kernel::index::{BtreeIndex, INDEX_VERSION, V4_INDEX_VERSION};
use redlinedb_kernel::storage::PageFile;
use redlinedb_sql::{Database, DbOptions};

use crate::lab::{Lab, error_class};

const SETUP: &str = "CREATE TABLE m(x, tag); \
    CREATE INDEX m_x ON m(x); \
    CREATE INDEX m_x_desc ON m(x DESC); \
    CREATE INDEX m_x_tag ON m(x, tag); \
    CREATE INDEX m_expr ON m(x * 2); \
    CREATE INDEX m_part ON m(x) WHERE tag > 'm'; \
    CREATE TABLE n(v NUMERIC UNIQUE, w REAL); \
    CREATE INDEX n_w ON n(w); \
    INSERT INTO m VALUES (2,'int'),(2.0,'real'),(1.5,'half'),(3,'three'),(1,'one'),\
    (9007199254740993,'big'),(9007199254740992.0,'bigreal'),(-1.5,'neg'),(0,'zero'),\
    (2.5,'twohalf'),(4.0,'four'); \
    INSERT INTO n VALUES (1, 1), (2.5, 2.5), (3.0, 3), (-7, -7.5);";

const CHECKS: &[(&str, bool)] = &[
    ("SELECT tag FROM m INDEXED BY m_x WHERE x = 2", false),
    ("SELECT tag FROM m INDEXED BY m_x WHERE x < 2", false),
    (
        "SELECT x FROM m INDEXED BY m_x WHERE x > 2 ORDER BY x",
        true,
    ),
    ("SELECT x FROM m INDEXED BY m_x ORDER BY x LIMIT 4", true),
    (
        "SELECT x FROM m INDEXED BY m_x_desc WHERE x > 2 ORDER BY x DESC",
        true,
    ),
    (
        "SELECT tag FROM m INDEXED BY m_x_tag WHERE x = 2 AND tag > 'a'",
        false,
    ),
    ("SELECT tag FROM m INDEXED BY m_expr WHERE x * 2 = 4", false),
    (
        "SELECT tag FROM m INDEXED BY m_part WHERE tag > 'm' AND x >= 2",
        false,
    ),
    ("SELECT v, typeof(v) FROM n WHERE v = 3", false),
    (
        "SELECT w FROM n INDEXED BY n_w WHERE w > -10 ORDER BY w",
        true,
    ),
    (
        "SELECT x FROM m NOT INDEXED WHERE x > 1.5 ORDER BY x, tag",
        true,
    ),
];

/// Index name -> (meta page, epoch) as the kernel finds them, opened
/// without the SQL layer so nothing is rebuilt.
fn kernel_view(path: &Path) -> (BTreeMap<String, (u64, u16)>, usize) {
    let engine = Engine::open(path, DbOptions::default().engine).expect("kernel open");
    let stale = engine.indexes_needing_rebuild().expect("stale list").len();
    let view = index_view(&engine);
    (view, stale)
}

/// Every index with a B-tree. (A UNIQUE or PRIMARY KEY column constraint's
/// `sqlite_autoindex_*` has none: it is enforced without one.)
fn index_view(engine: &Engine) -> BTreeMap<String, (u64, u16)> {
    engine
        .schema_snapshot()
        .indexes
        .iter()
        .filter_map(|index| {
            let meta = index.meta_page_id?;
            let epoch =
                BtreeIndex::format_version(engine.buffer_pool_for_tests(), meta).expect("epoch");
            Some((index.name.to_string(), (meta.0, epoch)))
        })
        .collect()
}

fn write_v4_database(path: &Path, setup: &str) {
    with_v4_index_format_for_tests(|| {
        let db = Database::create(path, DbOptions::default()).expect("create v4 database");
        db.connect().execute(setup).expect("v4 setup");
    });
}

fn assert_clean_equivalence(engine: &Arc<Engine>) {
    let report = engine.integrity_check_full().expect("full check");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    for relation in &report.relations {
        for index in &relation.indexes {
            assert_eq!(
                (index.index_minus_heap, index.structural_errors.len()),
                (0, 0),
                "index {} after the upgrade: {index:?}",
                index.index_name
            );
        }
    }
}

#[test]
fn opening_a_v4_database_rebuilds_every_index() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("v4.db");
    write_v4_database(&path, SETUP);

    // What 4.x left behind: every index at epoch 2 and, unopened, stale.
    let (v4, stale) = kernel_view(&path);
    assert_eq!(v4.len(), 6, "{v4:?}");
    assert!(
        v4.values().all(|&(_, epoch)| epoch == V4_INDEX_VERSION),
        "{v4:?}"
    );
    assert_eq!(stale, v4.len());

    // Opening through SQL rebuilds them all before any query runs.
    let lab = Lab::open(&path);
    lab.sqlite.execute_batch(SETUP).expect("sqlite setup");
    let engine = lab.redline.engine_for_tests();
    assert!(
        engine
            .indexes_needing_rebuild()
            .expect("stale list")
            .is_empty()
    );
    let v5 = index_view(&engine);
    assert_eq!(v5.keys().collect::<Vec<_>>(), v4.keys().collect::<Vec<_>>());
    for (name, &(meta, epoch)) in &v5 {
        assert_eq!(epoch, INDEX_VERSION, "{name} epoch");
        assert_ne!(meta, v4[name].0, "{name} still has its 4.x B-tree");
    }
    for (sql, ordered) in CHECKS {
        lab.assert_same(sql, *ordered);
    }
    assert_clean_equivalence(&engine);
    drop(engine);
    drop(lab);

    // Nothing was checkpointed, so the next open replays the WAL: the 4.x
    // B-trees' records (in the epoch-2 key format) must not reach the
    // rebuilt B-trees, and a second open rebuilds nothing.
    let lab = Lab::open(&path);
    lab.sqlite.execute_batch(SETUP).expect("sqlite setup");
    let engine = lab.redline.engine_for_tests();
    assert_eq!(index_view(&engine), v5, "the second open rebuilt again");
    for (sql, ordered) in CHECKS {
        lab.assert_same(sql, *ordered);
    }
    assert_clean_equivalence(&engine);
}

#[test]
fn v4_unique_duplicates_fail_the_upgrade_and_change_nothing() {
    // 4.x let a UNIQUE index hold both 1 and 1.0 (different key tags). The
    // current key space makes them one key, so the rebuild must fail loudly
    // instead of silently keeping one row.
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("dup.db");
    let setup = "CREATE TABLE u(y, z); CREATE UNIQUE INDEX u_y ON u(y); \
                 CREATE INDEX u_z ON u(z); \
                 INSERT INTO u VALUES (1, 'a'), (1.0, 'b'), (2, 'c');";
    write_v4_database(&path, setup);
    let (before, _) = kernel_view(&path);

    let err = Database::open(&path, DbOptions::default())
        .err()
        .expect("the upgrade must refuse a UNIQUE index with 1 and 1.0");
    let message = err.to_string();
    assert_eq!(error_class(&message), "unique constraint", "{message}");
    for needle in ["u_y", "1.0", "was not changed"] {
        assert!(
            message.contains(needle),
            "`{needle}` missing from: {message}"
        );
    }

    // Nothing committed: the indexes are still the 4.x B-trees at epoch 2,
    // and the database still opens as a 4.x database.
    let (after, stale) = kernel_view(&path);
    assert_eq!(after, before);
    assert_eq!(stale, 2);
    with_v4_index_format_for_tests(|| {
        let db = Database::open(&path, DbOptions::default()).expect("still a 4.x database");
        let conn = db.connect();
        let mut stmt = conn.prepare("SELECT count(*) FROM u").expect("prepare");
        assert!(matches!(
            stmt.step().expect("step"),
            redlinedb_sql::Step::Row
        ));
        assert_eq!(
            stmt.column_value(0).expect("count"),
            &redlinedb_sql::SqlValue::Integer(3)
        );
    });
}

/// The version gate of RedlineDB 4.1.0, verbatim from `rehydrate_index_handles`
/// in `crates/kernel/src/engine/catalog_ops/index.rs` (lines 136-151 at tag
/// v4.1.0, identical since v4.0.3): 2 opens, 1 is rebuilt, anything else
/// fails `Engine::open` with `UnsupportedVersion`, which 4.1.0 prints as
/// `unsupported format version: N`. The epoch rules are under "Decisions"
/// in `docs/launch/v5.0.0-evidence.md`.
fn v4_1_0_open_gate(epoch: u16) -> Result<(), redlinedb_kernel::Error> {
    match epoch {
        2 | 1 => Ok(()),
        other => Err(redlinedb_kernel::Error::UnsupportedVersion(other)),
    }
}

#[test]
fn a_v4_reader_refuses_a_v5_database_with_an_index() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("v5.db");
    {
        let db = Database::create(&path, DbOptions::default()).expect("create");
        db.connect().execute(SETUP).expect("setup");
    }
    let (view, stale) = kernel_view(&path);
    assert_eq!(stale, 0);
    for (name, &(_, epoch)) in &view {
        assert_eq!(epoch, INDEX_VERSION, "{name}");
        let err = v4_1_0_open_gate(epoch).expect_err("4.1.0 must refuse epoch 3");
        assert_eq!(err.to_string(), "unsupported format version: 3");
    }
}

#[test]
fn a_database_from_a_newer_epoch_is_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("future.db");
    let meta = {
        let db = Database::create(&path, DbOptions::default()).expect("create");
        let conn = db.connect();
        conn.execute("CREATE TABLE f(x); CREATE INDEX f_x ON f(x); INSERT INTO f VALUES (1);")
            .expect("setup");
        db.checkpoint().expect("checkpoint");
        conn.engine_for_tests().schema_snapshot().indexes[0]
            .meta_page_id
            .expect("physical index")
    };
    let config = DbOptions::default().engine;
    let page_file =
        PageFile::open(path.join(&config.data_file_name), config.page_size).expect("page file");
    let mut page = page_file.read_page(meta).expect("meta page");
    let future = INDEX_VERSION + 1;
    redlinedb_kernel::format::bytes::write_u16(
        page.special_bytes_mut().expect("special"),
        4,
        future,
    )
    .expect("stamp epoch");
    let lsn = page.header().expect("header").page_lsn;
    page.set_page_lsn(lsn).expect("reseal");
    page_file.write_page(&page).expect("write meta page");
    page_file.sync_data().expect("sync");
    drop(page_file);

    let err = Database::open(&path, DbOptions::default())
        .err()
        .expect("a newer epoch must be refused");
    assert!(
        err.to_string()
            .contains(&format!("unsupported format version: {future}")),
        "{err}"
    );
}
