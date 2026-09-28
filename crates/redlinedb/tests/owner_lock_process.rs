//! Two operating-system processes race for one database: a child process
//! owns it, and the parent's writable and read-only opens must fail with
//! `Busy` without changing any file of the child's image.
//!
//! This test lives in its own binary on purpose. `flock` belongs to the open
//! file description, and a spawned child holds a copy of every descriptor
//! from fork until exec closes it. A test in the same binary that drops a
//! database and reopens it while this test spawns its child can therefore
//! see `Busy` for that short window.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use redlinedb::Database;

#[path = "support/owner_image.rs"]
mod owner_image;

use owner_image::{assert_busy, count_rows, create_db_with_rows, read_only_options, snapshot};

const CHILD_DB_ENV: &str = "REDLINE_OWNER_LOCK_CHILD_DB";
// libtest prints `test owner_lock_holder_child ... ` without a newline before
// the child runs, so the parent looks for this token anywhere in a line.
const CHILD_READY: &str = "OWNER_LOCK_CHILD_READY";

/// Child half of `second_process_is_rejected_without_touching_the_image`.
/// It does nothing unless the parent set `REDLINE_OWNER_LOCK_CHILD_DB`.
#[test]
fn owner_lock_holder_child() {
    let Some(path) = std::env::var_os(CHILD_DB_ENV) else {
        return;
    };
    let path = PathBuf::from(path);
    let db = Database::open(&path).expect("child opens the database");
    db.connect()
        .expect("child connect")
        .execute("INSERT INTO t VALUES (100, 'child')", ())
        .expect("child insert");
    let mut stdout = std::io::stdout();
    writeln!(stdout, "{CHILD_READY}").expect("print READY");
    stdout.flush().expect("flush READY");
    let mut rest = String::new();
    let _ = std::io::stdin().read_line(&mut rest);
    drop(db);
}

#[test]
fn second_process_is_rejected_without_touching_the_image() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("two-process.redline");
    create_db_with_rows(&path);

    let mut child = Command::new(std::env::current_exe().expect("test binary path"))
        .args([
            "owner_lock_holder_child",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_DB_ENV, &path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn owner child");
    let stdout = child.stdout.take().expect("child stdout");
    let (lines_tx, lines_rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if lines_tx.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match lines_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Ok(line)) if line.contains(CHILD_READY) => break,
            Ok(Ok(_)) => continue,
            Ok(Err(err)) => panic!("reading child stdout: {err}"),
            Err(_) => {
                let _ = child.kill();
                let status = child.wait().expect("wait child");
                panic!("child never reported READY: {status}");
            }
        }
    }

    let before = snapshot(&path);
    assert_busy(
        Database::open(&path),
        "writable open while a process owns it",
    );
    assert_busy(
        Database::open_with_options(&path, read_only_options()),
        "read-only open while a process owns it",
    );
    assert_eq!(
        snapshot(&path),
        before,
        "a rejected open must not change the live owner's image"
    );

    drop(child.stdin.take());
    let status = child.wait().expect("wait child");
    reader.join().expect("child stdout reader");
    assert!(status.success(), "child failed: {status}");

    let db = Database::open(&path).expect("open after the child exits");
    assert_eq!(count_rows(&db), 4, "the child's committed row is kept");
}
