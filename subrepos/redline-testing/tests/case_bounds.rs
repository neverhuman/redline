//! Black-box bounds on one engine run of one case (SQ-09).
//!
//! Each test runs the real `redline-testing run` binary on one to three
//! corpus cases against a fake RedlineDB target: a bash script that answers
//! `--version` and the capability probes like a shell without optional
//! features, and does something hostile when it runs a case. The reference
//! is the `sqlite3` on PATH, as in `capability_probe_works`.
//!
//! A runner that cannot bound a case hangs; `RUN_DEADLINE` turns that hang
//! into a failure and kills the runner's process group.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

const RUN_DEADLINE: Duration = Duration::from_secs(60);

/// Answers `--version` as RedlineDB and fails every capability probe (they
/// pass `-batch`); only a case run (`--batch --bail <db>`) reaches the body.
const TARGET_PRELUDE: &str = r#"#!/usr/bin/env bash
case "$1" in
  --version) echo "redlinedb 0.0.0-case-bounds"; exit 0 ;;
  --batch) ;;
  *) cat >/dev/null; exit 1 ;;
esac
"#;

fn sqlite3() -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH is set");
    std::env::split_paths(&path)
        .map(|dir| dir.join("sqlite3"))
        .find(|candidate| candidate.is_file())
        .expect("sqlite3 must be on PATH (the reference of these runs)")
}

/// Unique per call, so concurrent tests never see each other's processes.
fn marker() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .subsec_nanos();
    format!("{}{nanos:09}", std::process::id())
}

struct Fixture {
    dir: tempfile::TempDir,
}

struct Run {
    status: ExitStatus,
    elapsed: Duration,
    stderr: String,
    raw: PathBuf,
    records: Vec<Value>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: tempfile::Builder::new()
                .prefix("redline-testing-case-bounds-")
                .tempdir()
                .expect("temp dir"),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn target(&self, body: &str) -> PathBuf {
        let path = self.path().join("fake-redlinedb");
        fs::write(&path, format!("{TARGET_PRELUDE}{body}\n")).expect("write fake target");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod fake target");
        wait_until_executable(&path);
        path
    }

    fn run(&self, target: &Path, case_ids: &[&str], extra: &[&str], env: &[(&str, &str)]) -> Run {
        let raw = self.path().join("raw.jsonl");
        let stderr_path = self.path().join("runner.stderr");
        let mut command = Command::new(env!("CARGO_BIN_EXE_redline-testing"));
        command
            .current_dir(self.path())
            .args(["run", "--suite", "sqlite_parity", "--target-bin"])
            .arg(target)
            .arg("--sqlite-bin")
            .arg(sqlite3())
            .args(["--workers", "1", "--repetitions", "1", "--warmup", "0"])
            .args(["--progress", "never", "--tmp-root"])
            .arg(self.path().join("tmp"))
            .arg("--output")
            .arg(&raw)
            .args(extra)
            .envs(env.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(&stderr_path).expect("runner stderr"))
            .process_group(0);
        for case_id in case_ids {
            command.args(["--case-id", case_id]);
        }
        let started = Instant::now();
        let mut child = command.spawn().expect("spawn redline-testing");
        let status = loop {
            if let Some(status) = child.try_wait().expect("poll redline-testing") {
                break status;
            }
            if started.elapsed() > RUN_DEADLINE {
                let _ = Command::new("kill")
                    .args(["-s", "KILL", "--", &format!("-{}", child.id())])
                    .status();
                let _ = child.wait();
                panic!(
                    "redline-testing did not finish within {RUN_DEADLINE:?}: the case was not bounded; stderr:\n{}",
                    fs::read_to_string(&stderr_path).unwrap_or_default()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let elapsed = started.elapsed();
        let records = fs::read_to_string(&raw)
            .unwrap_or_default()
            .lines()
            .map(|line| {
                serde_json::from_str(line)
                    .unwrap_or_else(|error| panic!("raw line is not a record ({error}): {line}"))
            })
            .collect();
        Run {
            status,
            elapsed,
            stderr: fs::read_to_string(&stderr_path).unwrap_or_default(),
            raw,
            records,
        }
    }
}

impl Run {
    /// The single record of a one-sample run of `case_id`.
    fn only_record(&self, case_id: &str) -> &Value {
        let matching = self
            .records
            .iter()
            .filter(|record| record["case_id"] == case_id)
            .collect::<Vec<_>>();
        assert_eq!(
            matching.len(),
            1,
            "expected one record for {case_id}; stderr:\n{}",
            self.stderr
        );
        matching[0]
    }

    fn completion_marker(&self) -> Option<Value> {
        let path = PathBuf::from(format!("{}.complete.json", self.raw.display()));
        let text = fs::read_to_string(path).ok()?;
        Some(serde_json::from_str(&text).expect("completion marker is JSON"))
    }
}

/// A child another test thread forked while `path` was open for writing
/// holds a write descriptor until it execs, and exec'ing `path` then fails
/// with ETXTBSY. Once one exec succeeds, none is left.
fn wait_until_executable(path: &Path) {
    for _ in 0..500 {
        match Command::new(path)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Err(error) if error.raw_os_error() == Some(26) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => return,
        }
    }
    panic!("{} stayed busy", path.display());
}

/// No process whose command line contains `pattern` is alive.
fn assert_no_survivors(pattern: &str) {
    let started = Instant::now();
    loop {
        let output = Command::new("pgrep")
            .args(["-f", pattern])
            .output()
            .expect("run pgrep");
        match output.status.code() {
            Some(1) => return,
            Some(0) if started.elapsed() < Duration::from_secs(5) => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => panic!(
                "processes matching {pattern:?} survived their case: {}",
                String::from_utf8_lossy(&output.stdout)
            ),
        }
    }
}

#[test]
fn sleeping_target_is_killed_at_the_case_deadline() {
    let fixture = Fixture::new();
    let target = fixture.target("exec sleep 30");
    let run = fixture.run(&target, &["00001"], &["--case-timeout-ms", "500"], &[]);
    assert!(
        run.elapsed < Duration::from_secs(20),
        "a 30 s sleep ran for {:?} under a 500 ms case timeout",
        run.elapsed
    );
    let record = run.only_record("00001");
    assert_eq!(record["status"], "failed", "{record}");
    assert_eq!(record["verdict_reason"], "execution_failure", "{record}");
    assert_eq!(record["execution_outcome"], "timeout", "{record}");
    assert_eq!(record["target_execution_outcome"], "timeout", "{record}");
    assert_eq!(record["reference_execution_outcome"], "exited", "{record}");
    assert!(
        record["diagnostic"]
            .as_str()
            .is_some_and(|text| text.contains("500 ms")),
        "{record}"
    );
    // The run itself completed: its failure is published, not lost.
    assert!(!run.status.success(), "an unlisted failure fails the run");
    let marker = run.completion_marker().expect("completion marker");
    assert_eq!(marker["records"], 1, "{marker}");
    assert_eq!(marker["suite"], "sqlite_parity", "{marker}");
}

#[test]
fn flooding_target_is_cut_off_at_the_output_cap() {
    let fixture = Fixture::new();
    let target = fixture.target("exec yes redline-flood");
    let run = fixture.run(&target, &["00001"], &["--max-output-bytes", "65536"], &[]);
    let record = run.only_record("00001");
    assert_eq!(record["status"], "failed", "{record}");
    assert_eq!(record["execution_outcome"], "output_limit", "{record}");
    assert_eq!(
        record["target_execution_outcome"], "output_limit",
        "{record}"
    );
    let artifact = fixture.path().join(
        record["artifact_dir"]
            .as_str()
            .expect("a failed sample writes an artifact"),
    );
    let captured = fs::metadata(artifact.join("redlinedb.stdout.txt"))
        .expect("captured target stdout")
        .len();
    assert_eq!(captured, 65_536, "the capture keeps exactly the cap");
}

#[path = "case_bounds/failures.rs"]
mod failures;
