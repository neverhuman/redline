//! `owner.lock` is held for as long as anything can still use the engine,
//! and every path that replaces a database directory takes it first.
//!
//! A `Connection` or `OwnedStatement` keeps the engine running after the
//! last `Database` handle drops. The lock used to belong to the handle, so
//! another process could take it and run recovery (which truncates the
//! WAL tail) under a live engine, and a second open in this process
//! started a second engine on the same files. `restore_from_backup` used
//! to delete the destination directory without asking whether anyone owned
//! it.

use std::fs::{File, TryLockError};
use std::path::Path;

use redlinedb::{
    Database, ErrorCode, OpenOptions, OwnedStep, PhysicalBackupOptions, RestoreOptions,
};

#[path = "support/owner_image.rs"]
mod owner_image;

use owner_image::{count_rows, create_db_with_rows, hold_foreign_lock, snapshot};

/// Whether another open file description could take `owner.lock` now, as
/// another process would. Releases it again at once.
fn lock_is_free(path: &Path) -> bool {
    let file = File::options()
        .read(true)
        .write(true)
        .open(path.join("owner.lock"))
        .expect("open owner.lock");
    match file.try_lock() {
        Ok(()) => true,
        Err(TryLockError::WouldBlock) => false,
        Err(TryLockError::Error(err)) => panic!("try_lock owner.lock: {err}"),
    }
}

#[test]
fn a_connection_keeps_the_lock_after_its_database_handle_drops() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("db");
    create_db_with_rows(&path);

    let mut conn = Database::open(&path)
        .expect("open")
        .connect()
        .expect("connect");
    assert!(
        !lock_is_free(&path),
        "owner.lock was released while a connection still uses the engine"
    );
    conn.execute("INSERT INTO t VALUES (?, ?)", (10_i64, "late"))
        .expect("the connection still writes");
    assert!(!lock_is_free(&path));

    // A second open in this process shares the live engine instead of
    // starting another one on the same files.
    let db = Database::open(&path).expect("reopen while the connection lives");
    conn.execute("INSERT INTO t VALUES (?, ?)", (11_i64, "shared"))
        .expect("insert");
    assert_eq!(
        count_rows(&db),
        5,
        "the second handle does not see the connection's commit"
    );
    drop(db);
    assert!(!lock_is_free(&path));

    drop(conn);
    assert!(
        lock_is_free(&path),
        "owner.lock stayed held after the last user dropped"
    );
}

#[test]
fn an_owned_statement_keeps_the_lock_after_its_connection_drops() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("db");
    create_db_with_rows(&path);

    let mut stmt = {
        let db = Database::open(&path).expect("open");
        let mut conn = db.connect().expect("connect");
        conn.prepare_owned("SELECT COUNT(*) FROM t")
            .expect("prepare")
    };
    assert!(
        !lock_is_free(&path),
        "owner.lock was released while a statement still uses the engine"
    );
    assert!(matches!(stmt.step().expect("step"), OwnedStep::Row));
    drop(stmt);
    assert!(lock_is_free(&path));
}

fn backup_of(path: &Path, dst: &Path) {
    let db = Database::open(path).expect("open source");
    db.backup_physical_to_path(dst, PhysicalBackupOptions::default())
        .expect("backup");
}

#[test]
fn restore_over_an_owned_directory_is_busy_and_changes_nothing() {
    let root = tempfile::tempdir().expect("tempdir");
    let source = root.path().join("source");
    create_db_with_rows(&source);
    let backup = root.path().join("backup");
    backup_of(&source, &backup);

    let target = root.path().join("target");
    create_db_with_rows(&target);
    let before = snapshot(&target);
    let foreign = hold_foreign_lock(&target);
    let err = Database::restore_from_backup(&backup, &target, RestoreOptions::default())
        .expect_err("restore over a directory another owner holds");
    assert_eq!(err.code(), ErrorCode::Busy, "{err}");
    assert_eq!(
        snapshot(&target),
        before,
        "a refused restore changed the owner's files"
    );
    drop(foreign);

    // Once nobody owns it, the restore replaces it.
    Database::restore_from_backup(&backup, &target, RestoreOptions::default())
        .expect("restore once the owner is gone");
    let restored = Database::open_with_options(&target, OpenOptions::default()).expect("open");
    assert_eq!(count_rows(&restored), 3);
}

/// A same-process open right after the last user of a database drops must
/// not report "another owner": the lock is still held for the moment the
/// closing engine needs to finish, and the open waits for that instead.
#[test]
fn reopen_while_the_last_user_drops_on_another_thread_waits_for_it() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("db");
    create_db_with_rows(&path);
    for round in 0..20 {
        let db = Database::open(&path).expect("open");
        let mut conn = db.connect().expect("connect");
        conn.execute("INSERT INTO t VALUES (?, ?)", (100 + round, "round"))
            .expect("insert");
        drop(db);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let dropper = {
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                drop(conn);
            })
        };
        barrier.wait();
        // Keep opening while the other thread closes the engine: each
        // attempt either shares the live engine or waits for it to close.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
        loop {
            let reopened = Database::open(&path)
                .unwrap_or_else(|err| panic!("round {round}: reopen failed: {err}"));
            drop(reopened);
            if std::time::Instant::now() > deadline {
                break;
            }
        }
        dropper.join().expect("dropper");
    }
    let db = Database::open(&path).expect("final open");
    assert_eq!(count_rows(&db), 23);
}

#[test]
fn sequential_close_and_reopen_of_one_path_never_reports_another_owner() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("db");
    create_db_with_rows(&path);
    for round in 0..20_i64 {
        let db = Database::open(&path).expect("open");
        let mut conn = db.connect().expect("connect");
        conn.execute("INSERT INTO t VALUES (?, ?)", (200 + round, "seq"))
            .expect("insert");
        // The handle goes first and the connection closes the engine.
        drop(db);
        drop(conn);
        assert!(
            lock_is_free(&path),
            "round {round}: lock still held after close"
        );
    }
}
