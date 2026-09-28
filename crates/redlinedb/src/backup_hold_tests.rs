//! A backup copies the database files while no checkpoint can run.
//!
//! Pressure checkpoints are on for every persistent database, so one can
//! start while a backup copies: it rewrites pages below the backup's
//! control generation and lands a newer control slot, and the copy mixes
//! the two. A checkpoint asked for during the copy must wait for it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::{BackupOptions, Database, Durability, OpenOptions, PhysicalBackupOptions};

fn database_with_rows(path: &std::path::Path) -> Database {
    // Normal: the test is about checkpoints, not about waiting for fsync.
    let options = OpenOptions {
        create: true,
        durability: Durability::Normal,
        ..OpenOptions::default()
    };
    let db = Database::open_with_options(path, options).expect("create");
    let mut conn = db.connect().expect("connect");
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)", ())
        .expect("create table");
    for id in 0..50_i64 {
        conn.execute("INSERT INTO t VALUES (?, ?)", (id, format!("row-{id}")))
            .expect("insert");
    }
    db
}

/// Run `backup` on its own thread; once it is about to copy, ask for a
/// checkpoint on another thread and report whether that checkpoint
/// finished before the copy did.
fn checkpoint_finished_during_copy(db: &Database, backup: impl FnOnce() + Send) -> bool {
    let (copying_tx, copying_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let go_rx = std::sync::Mutex::new(Some(go_rx));
    let finished = AtomicBool::new(false);
    let finished_during_copy = AtomicBool::new(false);
    thread::scope(|scope| {
        scope.spawn(|| {
            let go_rx = go_rx.lock().expect("go").take().expect("go receiver");
            crate::snapshot::set_before_copy_hook(Some(Box::new(move || {
                copying_tx.send(()).expect("signal copying");
                go_rx.recv().expect("wait for go");
            })));
            backup();
            crate::snapshot::set_before_copy_hook(None);
        });
        copying_rx
            .recv()
            .expect("the backup never reached its copy");
        let checkpoint = scope.spawn(|| {
            db.checkpoint().expect("checkpoint");
            finished.store(true, Ordering::SeqCst);
        });
        thread::sleep(Duration::from_millis(300));
        finished_during_copy.store(finished.load(Ordering::SeqCst), Ordering::SeqCst);
        go_tx.send(()).expect("release the copy");
        checkpoint.join().expect("checkpoint thread");
    });
    assert!(finished.load(Ordering::SeqCst), "the checkpoint never ran");
    finished_during_copy.load(Ordering::SeqCst)
}

#[test]
fn a_checkpoint_waits_while_a_physical_backup_copies() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = database_with_rows(&dir.path().join("db"));
    let dst = dir.path().join("physical");
    let ran_during_copy = checkpoint_finished_during_copy(&db, || {
        db.backup_physical_to_path(&dst, PhysicalBackupOptions::default())
            .expect("physical backup");
    });
    assert!(!ran_during_copy, "a checkpoint ran while the backup copied");
}

#[test]
fn a_checkpoint_waits_while_a_snapshot_backup_copies() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = database_with_rows(&dir.path().join("db"));
    let dst = dir.path().join("snapshot");
    let ran_during_copy = checkpoint_finished_during_copy(&db, || {
        db.backup_to_path(&dst, BackupOptions::default())
            .expect("snapshot backup");
    });
    assert!(!ran_during_copy, "a checkpoint ran while the backup copied");
}
