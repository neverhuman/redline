//! A recovery child's READY line must imply a committed, ledgered transaction.
//! The kill timer may fire before this child gets another scheduling slice.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn ready_has_first_ack(engine: &str) {
    let directory = tempfile::tempdir().unwrap();
    let ledger = directory.path().join("ack.log");
    let mut command = Command::new(env!("CARGO_BIN_EXE_redlinedb-bench"));
    // Keep compiler output inside this lane's target. The preload applies only
    // to our child and makes the parent regression deterministic on Linux.
    #[cfg(target_os = "linux")]
    let (_hook_directory, marker, release) = {
        let scratch = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
        std::fs::create_dir_all(scratch).unwrap();
        let hook = tempfile::tempdir_in(scratch).unwrap();
        let library = hook.path().join("ready-delay.so");
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/support/recover_ready_delay.c");
        assert!(
            Command::new("cc")
                .args(["-shared", "-fPIC", "-O2"])
                .arg(source)
                .arg("-o")
                .arg(&library)
                .status()
                .unwrap()
                .success()
        );
        let marker = hook.path().join("marker");
        let release = hook.path().join("release");
        command
            .env("LD_PRELOAD", &library)
            .env("REDLINE_READY_TEST_HOOK_MARKER", &marker)
            .env("REDLINE_READY_TEST_HOOK_RELEASE", &release);
        (hook, marker, release)
    };
    let mut child = OwnedChild(
        command
            .args([
                "recover-child",
                "--engine",
                engine,
                "--durability",
                "strict",
            ])
            .arg("--db-dir")
            .arg(directory.path().join("database"))
            .arg("--ack-log")
            .arg(&ledger)
            .args(["--rows", "1000000"])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout).read_line(&mut line);
        let _ = sender.send((result, line));
    });
    let (read, line) = receiver
        .recv_timeout(Duration::from_secs(120))
        .expect("child must become ready within the harness startup timeout");
    read.unwrap();
    assert_eq!(line.trim_end(), "READY");
    #[cfg(target_os = "linux")]
    assert_eq!(
        std::fs::read_to_string(marker).unwrap(),
        "READY hook reached\n"
    );
    // Read while the child is live: READY must already establish the first
    // completed line, independently of any later transaction or CPU time.
    let mut first = String::new();
    BufReader::new(std::fs::File::open(ledger).unwrap())
        .read_line(&mut first)
        .unwrap();
    assert!(
        first.ends_with('\n'),
        "READY preceded a complete acknowledgement"
    );
    assert!(first.starts_with("0\t"), "first acknowledgement: {first:?}");
    #[cfg(target_os = "linux")]
    std::fs::write(release, "acknowledgement checked\n").unwrap();
}

#[test]
fn redline_ready_has_a_completed_acknowledgement() {
    ready_has_first_ack("redline");
}

#[test]
fn sqlite_ready_has_a_completed_acknowledgement() {
    ready_has_first_ack("sqlite");
}
