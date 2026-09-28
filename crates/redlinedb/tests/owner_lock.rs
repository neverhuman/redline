//! Process ownership (`owner.lock`) is taken before an open reads or repairs
//! the durable image.
//!
//! A second opener that loses the ownership race must get `Busy` without
//! having run recovery. Recovery truncates a torn WAL tail, creates missing
//! sidecars and rewrites index pages, so an open that ran recovery before it
//! noticed the owner has already changed the owner's files.
//!
//! `flock` locks belong to an open file description, so a second
//! `File::open` of `owner.lock` in this process conflicts with the engine's
//! lock the same way another process would. These tests use that to act as a
//! foreign owner; `owner_lock_process.rs` uses a real second process.

use std::fs;

use redlinedb::{Database, ErrorCode, OpenOptions};
use sha2::{Digest, Sha256};

#[path = "support/owner_image.rs"]
mod owner_image;

use owner_image::{
    append_torn_wal_tail, assert_busy, count_rows, create_db_with_rows, hold_foreign_lock,
    read_only_options, snapshot,
};

#[test]
fn rejected_open_leaves_image_unchanged() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("owned.redline");
    create_db_with_rows(&path);
    let segment = append_torn_wal_tail(&path);
    let torn_len = fs::metadata(&segment).expect("segment metadata").len();
    let before = snapshot(&path);

    let foreign = hold_foreign_lock(&path);
    assert_busy(Database::open(&path), "writable open");
    assert_eq!(
        fs::metadata(&segment).expect("segment metadata").len(),
        torn_len,
        "a rejected open must not truncate the owner's WAL"
    );
    assert_busy(
        Database::open_with_options(&path, read_only_options()),
        "read-only open",
    );
    assert_busy(Database::create(&path), "create over an owned image");
    assert_eq!(
        snapshot(&path),
        before,
        "a rejected open must not change any file of the owner's image"
    );

    drop(foreign);
    let db = Database::open(&path).expect("open after the owner releases");
    assert_eq!(count_rows(&db), 3);
    // The fixture is live: an owning open does cut the torn tail, so the
    // length check above would have caught a rejected open that recovered.
    assert_eq!(
        fs::metadata(&segment).expect("segment metadata").len(),
        torn_len - 37,
        "an owning open should truncate the torn tail"
    );
}

#[cfg(unix)]
#[test]
fn symlink_alias_rejected_while_owned() {
    let root = tempfile::tempdir().expect("tempdir");
    let real = root.path().join("real.redline");
    let alias = root.path().join("alias.redline");
    create_db_with_rows(&real);
    append_torn_wal_tail(&real);
    std::os::unix::fs::symlink(&real, &alias).expect("symlink alias");
    let before = snapshot(&real);

    let foreign = hold_foreign_lock(&real);
    assert_busy(Database::open(&alias), "writable open through an alias");
    assert_busy(
        Database::open_with_options(&alias, read_only_options()),
        "read-only open through an alias",
    );
    assert_eq!(
        snapshot(&real),
        before,
        "an open through an alias must not change the owner's image"
    );

    drop(foreign);
    let through_alias = Database::open(&alias).expect("open through the alias");
    let direct = Database::open(&real).expect("open the real path in the same process");
    assert_eq!(through_alias.path(), direct.path());
    assert_eq!(count_rows(&direct), 3);
}

#[test]
fn create_with_only_stale_owner_lock_creates_fresh_db() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("stale.redline");
    fs::create_dir_all(&path).expect("database directory");
    fs::write(path.join("owner.lock"), b"").expect("stale owner.lock");

    {
        let db = Database::create(&path).expect("create over a stale owner.lock");
        let mut conn = db.connect().expect("connect");
        conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)", ())
            .expect("create table");
        conn.execute("INSERT INTO t VALUES (1, 'fresh')", ())
            .expect("insert row");
    }

    assert!(path.join("data.redline").exists(), "a fresh image exists");
    let db = Database::open(&path).expect("reopen");
    assert_eq!(count_rows(&db), 1);
}

#[test]
fn create_on_existing_image_keeps_rows() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("existing.redline");
    create_db_with_rows(&path);

    let db = Database::create(&path).expect("create on an existing image opens it");
    assert_eq!(
        count_rows(&db),
        3,
        "create must not replace an existing image"
    );
}

#[test]
fn readonly_handle_cannot_write_after_writable_open() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("shared.redline");
    let writer = Database::create(&path).expect("writable open");
    writer
        .connect()
        .expect("connect")
        .execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)", ())
        .expect("create table");

    let reader = Database::open_with_options(&path, read_only_options()).expect("read-only open");
    let mut conn = reader.connect().expect("read-only connect");
    let err = conn
        .execute("INSERT INTO t VALUES (1, 'from the read-only handle')", ())
        .expect_err("a read-only handle must not write");
    assert_eq!(err.code(), ErrorCode::ReadOnly, "{err}");
    let err = reader
        .clone()
        .connect()
        .expect("connect from a clone")
        .execute("DELETE FROM t", ())
        .expect_err("a clone of a read-only handle stays read-only");
    assert_eq!(err.code(), ErrorCode::ReadOnly, "{err}");
    assert_eq!(count_rows(&reader), 0);

    writer
        .connect()
        .expect("connect")
        .execute("INSERT INTO t VALUES (1, 'from the writer')", ())
        .expect("the writable handle still writes");
    assert_eq!(count_rows(&reader), 1);
}

#[test]
fn named_ephemeral_session_owned_elsewhere_is_not_deleted() {
    let root = tempfile::tempdir().expect("tempdir");
    let session = "owner-lock-named-session";
    let session_dir = root.path().join(format!(
        "redlinedb-ephemeral-{:x}",
        Sha256::digest(session.as_bytes())
    ));
    fs::create_dir_all(&session_dir).expect("session directory");
    fs::write(session_dir.join("live-marker"), b"live").expect("marker");
    fs::write(session_dir.join("owner.lock"), b"").expect("owner.lock");
    let options = OpenOptions::default().with_temp_dir(root.path());

    let foreign = hold_foreign_lock(&session_dir);
    match Database::create_ephemeral(session, options.clone()) {
        Ok(db) => panic!(
            "session {} opened while another process owns it",
            db.path().display()
        ),
        Err(err) => assert_eq!(err.code(), ErrorCode::Busy, "{err}"),
    }
    assert_eq!(
        fs::read(session_dir.join("live-marker")).expect("marker survives"),
        b"live",
        "an owned session directory must not be deleted"
    );

    drop(foreign);
    let db = Database::create_ephemeral(session, options).expect("replace the stale session");
    assert_eq!(db.path(), session_dir.as_path());
    assert!(
        !session_dir.join("live-marker").exists(),
        "a stale session is replaced by a fresh one"
    );
    drop(db);
    assert!(!session_dir.exists(), "the session is removed on last drop");
}
