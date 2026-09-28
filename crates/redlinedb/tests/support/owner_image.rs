//! Helpers shared by the owner-lock tests: build a database image, damage
//! its WAL tail, snapshot every file, and hold `owner.lock` from outside the
//! engine.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use redlinedb::{Database, ErrorCode, OpenOptions};
use sha2::{Digest, Sha256};

const TORN_TAIL: [u8; 37] = [0xA5; 37];

/// (length, sha256) for every file under the database directory except
/// `owner.lock`, and a marker for every directory.
pub type ImageSnapshot = BTreeMap<PathBuf, (u64, String)>;

pub fn snapshot(root: &Path) -> ImageSnapshot {
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn walk(root: &Path, dir: &Path, out: &mut ImageSnapshot) {
    for entry in fs::read_dir(dir).expect("read database directory") {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .expect("relative path")
            .to_path_buf();
        let file_type = entry.file_type().expect("file type");
        if file_type.is_dir() {
            out.insert(relative, (0, "<dir>".to_string()));
            walk(root, &path, out);
        } else if relative != Path::new("owner.lock") {
            let bytes = fs::read(&path).expect("read image file");
            let digest = format!("{:x}", Sha256::digest(&bytes));
            out.insert(relative, (bytes.len() as u64, digest));
        }
    }
}

pub fn create_db_with_rows(path: &Path) {
    let db = Database::create(path).expect("create database");
    let mut conn = db.connect().expect("connect");
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)", ())
        .expect("create table");
    for id in 1..=3_i64 {
        conn.execute("INSERT INTO t VALUES (?, ?)", (id, format!("row-{id}")))
            .expect("insert row");
    }
}

pub fn last_wal_segment(path: &Path) -> PathBuf {
    let mut segments: Vec<PathBuf> = fs::read_dir(path.join("wal"))
        .expect("read wal directory")
        .map(|entry| entry.expect("wal entry").path())
        .filter(|p| p.extension().and_then(|ext| ext.to_str()) == Some("wal"))
        .collect();
    segments.sort();
    segments.pop().expect("at least one wal segment")
}

/// Leave a torn tail on the newest WAL segment. Recovery truncates it with
/// `set_len`, so an open that recovers before it checks ownership shrinks
/// the file.
pub fn append_torn_wal_tail(path: &Path) -> PathBuf {
    let segment = last_wal_segment(path);
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&segment)
        .expect("open last wal segment");
    file.write_all(&TORN_TAIL).expect("append torn tail");
    file.sync_all().expect("sync torn tail");
    segment
}

/// Take `owner.lock` through a separate open file description, the way a
/// process that owns the database holds it.
pub fn hold_foreign_lock(path: &Path) -> File {
    let file = File::options()
        .read(true)
        .write(true)
        .open(path.join("owner.lock"))
        .expect("open owner.lock");
    file.try_lock().expect("take the foreign owner lock");
    file
}

pub fn read_only_options() -> OpenOptions {
    OpenOptions::default()
        .with_create(false)
        .with_read_only(true)
}

pub fn assert_busy(result: redlinedb::Result<Database>, what: &str) {
    match result {
        Ok(_) => panic!("{what}: open succeeded while another owner holds owner.lock"),
        Err(err) => assert_eq!(err.code(), ErrorCode::Busy, "{what}: {err}"),
    }
}

pub fn count_rows(db: &Database) -> i64 {
    db.connect()
        .expect("connect")
        .query_row("SELECT COUNT(*) FROM t", ())
        .expect("count rows")
}
