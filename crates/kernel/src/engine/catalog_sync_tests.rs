//! Catalog fsync follows the live commit durability, not the open-time one.
//!
//! The sync counters are per thread, so each test counts only the saves it
//! makes itself while other lib tests save Strict catalogs in parallel.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;

use super::{CommitDurability, CommitOutcome, Engine, EngineConfig};
use crate::catalog::{
    ColumnSpec, CreateTableSpec, DbName, catalog_dir_syncs, catalog_metadata_syncs,
};
use crate::format::RelId;
use crate::txn::Isolation;

fn engine(dir: &Path, durability: CommitDurability) -> Arc<Engine> {
    Engine::create(
        dir,
        EngineConfig {
            rel_id: RelId(1),
            commit_durability: durability,
            busy_timeout: Duration::from_millis(50),
            ..EngineConfig::default()
        },
    )
    .unwrap()
}

/// (schema-file fsyncs, parent-directory fsyncs) made by this thread.
fn syncs() -> (u64, u64) {
    let dir = catalog_dir_syncs();
    (catalog_metadata_syncs() - dir, dir)
}

fn syncs_since(before: (u64, u64)) -> (u64, u64) {
    let now = syncs();
    (now.0 - before.0, now.1 - before.1)
}

fn save_current_catalog(engine: &Engine) {
    engine
        .catalog_store
        .save_atomic(engine.catalog.current().as_ref())
        .unwrap();
}

fn commit_create_table(engine: &Engine, name: &str) {
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    engine
        .create_table(
            &mut tx,
            CreateTableSpec {
                schema: None,
                name: DbName::new(name),
                if_not_exists: false,
                columns: vec![ColumnSpec {
                    name: DbName::new("v"),
                    declared_type: Some("INTEGER".to_owned()),
                    constraints: vec![],
                    collation: None,
                    default_value: None,
                    autoincrement: false,
                    generated: None,
                }],
                constraints: vec![],
                strict: false,
                without_rowid: false,
                normalized_sql: Some(format!("CREATE TABLE {name} (v INTEGER)")),
            },
        )
        .unwrap();
    assert!(matches!(
        engine.commit(tx).unwrap(),
        CommitOutcome::Committed(_)
    ));
}

#[test]
fn catalog_fsync_follows_a_later_durability_change() {
    let dir = TempDir::new().unwrap();
    let engine = engine(dir.path(), CommitDurability::Strict);
    for (durability, expected) in [
        (CommitDurability::Normal, (0, 0)),
        (CommitDurability::UnsafeDev, (0, 0)),
        (CommitDurability::Strict, (1, 1)),
    ] {
        engine.set_commit_durability(durability);
        let before = syncs();
        save_current_catalog(&engine);
        assert_eq!(syncs_since(before), expected, "{durability:?}");
    }
}

#[test]
fn catalog_sync_count_ignores_saves_on_other_threads() {
    let dir = TempDir::new().unwrap();
    let engine = engine(dir.path(), CommitDurability::Strict);
    let before = catalog_metadata_syncs();
    let other_thread_syncs = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let start = catalog_metadata_syncs();
                save_current_catalog(&engine);
                catalog_metadata_syncs() - start
            })
            .join()
            .unwrap()
    });
    assert_eq!(
        catalog_metadata_syncs(),
        before,
        "a Strict save on another thread was counted on this one"
    );
    assert_eq!(other_thread_syncs, 2);
}

#[test]
fn ddl_commit_fsyncs_the_catalog_only_under_live_strict() {
    let dir = TempDir::new().unwrap();
    let engine = engine(dir.path(), CommitDurability::Normal);
    engine.set_commit_durability(CommitDurability::Strict);
    let before = syncs();
    commit_create_table(&engine, "t1");
    assert_eq!(syncs_since(before), (1, 1));
    engine.set_commit_durability(CommitDurability::Normal);
    let before = syncs();
    commit_create_table(&engine, "t2");
    assert_eq!(syncs_since(before), (0, 0));
    // Normal still writes the schema file; it only skips the fsyncs.
    let saved = engine.catalog_store.load().unwrap().unwrap();
    assert!(saved.tables.iter().any(|table| &*table.name == "t2"));
}
