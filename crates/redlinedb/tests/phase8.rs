use std::path::Path;

use redlinedb::{
    ArchiveMode, Database, Durability, Lsn, OpenOptions, PhysicalBackupOptions, RecoveryTarget,
    RestoreOptions, SlotKind, Step, ValueRef,
};
use sha2::{Digest, Sha256};
use tempfile::tempdir;

#[test]
fn physical_backup_restore_roundtrip() {
    let dir = tempdir().expect("tempdir");
    let src = dir.path().join("src.db");
    let backup = dir.path().join("backup.db");
    let dst = dir.path().join("restore.db");

    let db = Database::create(&src).expect("create db");
    let mut conn = db.connect().expect("connect");
    conn.execute("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT)", ())
        .expect("create table");
    conn.execute("INSERT INTO items VALUES (1, 'one')", ())
        .expect("insert 1");
    conn.execute("INSERT INTO items VALUES (2, 'two')", ())
        .expect("insert 2");

    let backup_stats = db
        .backup_physical_to_path(
            &backup,
            PhysicalBackupOptions {
                include_wal: true,
                archive_mode: ArchiveMode::Off,
            },
        )
        .expect("backup");
    assert!(backup_stats.files_copied > 0);
    assert!(backup_stats.bytes_copied > 0);

    let restore_stats =
        Database::restore_from_backup(&backup, &dst, RestoreOptions::default()).expect("restore");
    assert!(restore_stats.files_copied > 0);
    assert!(restore_stats.bytes_copied > 0);

    let restored = Database::open_with_options(
        &dst,
        OpenOptions {
            create: false,
            ..Default::default()
        },
    )
    .expect("open restored");
    let mut conn = restored.connect().expect("connect restored");
    let mut rows = conn
        .query("SELECT name FROM items ORDER BY id", ())
        .expect("query");
    let mut names = Vec::new();
    while let Step::Row(row) = rows.step().expect("step") {
        match row.get_ref(0).expect("ref") {
            ValueRef::Text(value) => names.push(value.to_owned()),
            other => panic!("unexpected value: {other:?}"),
        }
    }
    assert_eq!(names, vec!["one".to_owned(), "two".to_owned()]);
}

#[test]
fn slots_are_persisted_and_listed() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("slots.db");
    let db = Database::create(&path).expect("create db");

    let physical = db
        .create_physical_slot("physical-a")
        .expect("physical slot");
    let logical = db.create_logical_slot("logical-a").expect("logical slot");
    assert_eq!(physical.kind, SlotKind::Physical);
    assert_eq!(logical.kind, SlotKind::Logical);

    let slots = db.replication_slots().expect("replication slots");
    assert_eq!(slots.len(), 2);
    assert!(slots.iter().any(|slot| slot.name == "physical-a"));
    assert!(slots.iter().any(|slot| slot.name == "logical-a"));

    db.drop_replication_slot("physical-a")
        .expect("drop physical slot");
    let slots = db.replication_slots().expect("replication slots");
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].name, "logical-a");
}

#[test]
fn archive_and_retention_stats_are_available() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("stats.db");
    let db = Database::create(&path).expect("create db");
    let mut conn = db.connect().expect("connect");
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)", ())
        .expect("create table");
    conn.execute("INSERT INTO t VALUES (1, 'x')", ())
        .expect("insert");

    let archive = db.archive_stats().expect("archive stats");
    assert_eq!(archive.archive_mode, ArchiveMode::Off);

    let retention = db.retention_horizon().expect("retention");
    assert!(retention.catalog_csn >= retention.vacuum_csn);
}

fn item_names(db: &Database) -> Vec<String> {
    let mut conn = db.connect().expect("connect");
    let mut rows = conn
        .query("SELECT name FROM items ORDER BY id", ())
        .expect("query");
    let mut names = Vec::new();
    while let Step::Row(row) = rows.step().expect("step") {
        match row.get_ref(0).expect("ref") {
            ValueRef::Text(value) => names.push(value.to_owned()),
            other => panic!("unexpected value: {other:?}"),
        }
    }
    names
}

fn open_existing(path: &Path) -> Database {
    Database::open_with_options(
        path,
        OpenOptions {
            create: false,
            ..Default::default()
        },
    )
    .expect("open existing")
}

/// Stand in for an online backup that copied the WAL after more commits
/// landed: replace the backup's WAL segments with the source's current
/// ones, and record the new file list and tree hash in its manifest the
/// way `backup_physical_to_path` computes them.
fn extend_backup_wal(src: &Path, backup: &Path) {
    let manifest_path = backup.join("phase8").join("backup-manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).expect("read manifest"))
            .expect("parse manifest");
    let mut files: Vec<String> = manifest["files"]
        .as_array()
        .expect("manifest files")
        .iter()
        .map(|value| value.as_str().expect("file name").to_owned())
        .collect();
    for entry in std::fs::read_dir(src.join("wal")).expect("read wal") {
        let entry = entry.expect("wal entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".wal") {
            continue;
        }
        std::fs::copy(entry.path(), backup.join("wal").join(&name)).expect("copy segment");
        let rel = format!("wal/{name}");
        if !files.contains(&rel) {
            files.push(rel);
        }
    }
    files.sort();
    let mut hasher = Sha256::new();
    for rel in &files {
        hasher.update(rel.as_bytes());
        hasher.update(std::fs::read(backup.join(rel)).expect("read backup file"));
    }
    manifest["files"] = serde_json::json!(files);
    manifest["tree_hash"] = serde_json::json!(format!("{:x}", hasher.finalize()));
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("encode manifest"),
    )
    .expect("write manifest");
}

#[test]
fn restore_to_lsn_stays_at_target_after_reopen() {
    let dir = tempdir().expect("tempdir");
    let src = dir.path().join("src.db");
    let backup = dir.path().join("backup.db");
    let dst = dir.path().join("restore.db");

    let db = Database::open_with_options(
        &src,
        OpenOptions {
            create: true,
            durability: Durability::Strict,
            ..Default::default()
        },
    )
    .expect("create db");
    let mut conn = db.connect().expect("connect");
    conn.execute("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT)", ())
        .expect("create table");
    conn.execute("INSERT INTO items VALUES (1, 'one')", ())
        .expect("insert 1");
    conn.execute("INSERT INTO items VALUES (2, 'two')", ())
        .expect("insert 2");
    db.backup_physical_to_path(&backup, PhysicalBackupOptions::default())
        .expect("backup");
    conn.execute("INSERT INTO items VALUES (3, 'three')", ())
        .expect("insert 3");
    let target = db.stats().expect("stats").wal_durable_lsn;
    conn.execute("INSERT INTO items VALUES (4, 'four')", ())
        .expect("insert 4");
    extend_backup_wal(&src, &backup);
    drop(conn);
    drop(db);

    Database::restore_from_backup(
        &backup,
        &dst,
        RestoreOptions {
            target: RecoveryTarget::Lsn(Lsn(target)),
            preserve_timeline: false,
        },
    )
    .expect("restore to lsn");

    // The first ordinary open after the restore must not replay the WAL
    // the restore stopped short of.
    let restored = open_existing(&dst);
    assert_eq!(item_names(&restored), ["one", "two", "three"]);
    let mut conn = restored.connect().expect("connect restored");
    conn.execute("INSERT INTO items VALUES (5, 'five')", ())
        .expect("insert 5");
    drop(conn);
    drop(restored);

    let reopened = open_existing(&dst);
    assert_eq!(item_names(&reopened), ["one", "two", "three", "five"]);
}
