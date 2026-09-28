//! Directory durability of the database root and its `wal` entry.
//!
//! `take_synced_dirs` lists the directories this thread fsynced through
//! `StdFileSystem::sync_dir`. WAL creation runs on the calling thread, so
//! the list covers everything `Engine::create` and `Engine::open` sync.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::{Engine, EngineConfig};
use crate::io::take_synced_dirs;

fn small_config() -> EngineConfig {
    EngineConfig {
        buffer_pool_pages: 16,
        ..EngineConfig::default()
    }
}

fn index_of(synced: &[PathBuf], dir: &Path) -> usize {
    synced
        .iter()
        .position(|seen| seen == dir)
        .unwrap_or_else(|| panic!("{} never synced: {synced:#?}", dir.display()))
}

/// Creating `wal` syncs the root for the new entry, then creates the first
/// segment and syncs `wal`. So the root sync directly precedes the first
/// `wal` sync, and it happens after `wal` exists.
fn assert_root_synced_for_wal_entry(synced: &[PathBuf], root: &Path) {
    let wal = index_of(synced, &root.join("wal"));
    assert!(
        wal > 0 && synced[wal - 1] == root,
        "root not synced right after creating wal/: {synced:#?}"
    );
}

#[test]
fn persistent_create_syncs_the_new_root_its_parent_and_the_wal_entry() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("db");
    let _ = take_synced_dirs();
    let engine = Engine::create(&root, small_config()).unwrap();
    let synced = take_synced_dirs();
    drop(engine);

    // `db` is a new name in its parent.
    let parent = index_of(&synced, tmp.path());
    assert!(parent < index_of(&synced, &root), "{synced:#?}");
    assert_root_synced_for_wal_entry(&synced, &root);
}

#[test]
fn open_that_recreates_the_wal_dir_syncs_the_root() {
    let tmp = TempDir::new().unwrap();
    drop(Engine::create(tmp.path(), small_config()).unwrap());
    std::fs::remove_dir_all(tmp.path().join("wal")).unwrap();

    let _ = take_synced_dirs();
    let engine = Engine::open(tmp.path(), small_config()).unwrap();
    let synced = take_synced_dirs();
    drop(engine);

    assert_root_synced_for_wal_entry(&synced, tmp.path());
    // The root already existed, so its parent is not synced again.
    let parent = tmp.path().parent().unwrap();
    assert!(!synced.iter().any(|seen| seen == parent), "{synced:#?}");
}

#[test]
fn volatile_create_syncs_no_directory() {
    let tmp = TempDir::new().unwrap();
    let _ = take_synced_dirs();
    let engine = Engine::create_volatile(tmp.path(), small_config()).unwrap();
    assert_eq!(take_synced_dirs(), Vec::<PathBuf>::new());
    drop(engine);
}
