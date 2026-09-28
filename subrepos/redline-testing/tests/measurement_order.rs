//! Engine order within a sample (BM3-04).
//!
//! Each test runs the real `redline-testing run` on one corpus case with one
//! warmup and three measured repetitions. The reference is the `sqlite3` on
//! PATH behind a wrapper, and the target is a fake RedlineDB; both append
//! their name to one log each time they run a case, so the log shows which
//! engine ran first in each sample. The raw records must say the same.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::Value;

/// Answers `--version` as RedlineDB and fails every capability probe; only
/// a case run (`--batch --bail <db>`) is logged.
const TARGET: &str = r#"#!/usr/bin/env bash
case "$1" in
  --version) echo "redlinedb 0.0.0-measurement-order"; exit 0 ;;
  --batch) ;;
  *) cat >/dev/null; exit 1 ;;
esac
echo target >> "$ORDER_LOG"
cat >/dev/null
exit 0
"#;

fn sqlite3() -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH is set");
    std::env::split_paths(&path)
        .map(|dir| dir.join("sqlite3"))
        .find(|candidate| candidate.is_file())
        .expect("sqlite3 must be on PATH (the reference of these runs)")
}

fn executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod script");
    wait_until_executable(path);
}

/// A child another test thread forked while `path` was open for writing
/// holds a write descriptor until it execs; exec'ing `path` then fails with
/// ETXTBSY. Once one exec succeeds, none is left.
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

struct Run {
    code: Option<i32>,
    stderr: String,
    /// The engine each case run logged, in order.
    case_runs: Vec<String>,
    records: Vec<Value>,
    manifest: Option<Value>,
}

fn run(order: Option<&str>) -> Run {
    let dir = tempfile::Builder::new()
        .prefix("redline-testing-order-")
        .tempdir()
        .expect("temp dir");
    let log = dir.path().join("order.log");
    let reference = dir.path().join("sqlite3");
    executable(
        &reference,
        &format!(
            "#!/usr/bin/env bash\nif [ \"$1\" != --version ]; then echo reference >> \"$ORDER_LOG\"; fi\nexec {} \"$@\"\n",
            sqlite3().display()
        ),
    );
    let target = dir.path().join("fake-redlinedb");
    executable(&target, TARGET);
    let raw = dir.path().join("out").join("raw.jsonl");
    let mut command = Command::new(env!("CARGO_BIN_EXE_redline-testing"));
    command
        .current_dir(dir.path())
        .env("ORDER_LOG", &log)
        .args(["run", "--suite", "sqlite_parity", "--case-id", "00001"])
        .arg("--target-bin")
        .arg(&target)
        .arg("--sqlite-bin")
        .arg(&reference)
        .args(["--workers", "1", "--warmup", "1", "--repetitions", "3"])
        .args(["--progress", "never", "--tmp-root"])
        .arg(dir.path().join("tmp"))
        .arg("--output")
        .arg(&raw)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    if let Some(order) = order {
        command.args(["--order", order]);
    }
    let output = command.output().expect("run redline-testing");
    // The reference also logs its capability probes, which all run before
    // any case; the case runs are the last two lines per sample.
    let logged = fs::read_to_string(&log).unwrap_or_default();
    let logged = logged.lines().map(str::to_owned).collect::<Vec<_>>();
    let case_runs = logged[logged.len().saturating_sub(8)..].to_vec();
    let records = fs::read_to_string(&raw)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("raw line is JSON"))
        .collect();
    let manifest = fs::read_to_string(raw.with_file_name("manifest.json"))
        .ok()
        .map(|text| serde_json::from_str(&text).expect("manifest is JSON"));
    Run {
        code: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        case_runs,
        records,
        manifest,
    }
}

fn pairs(order: &[(&str, &str)]) -> Vec<String> {
    order
        .iter()
        .flat_map(|(first, second)| [first.to_string(), second.to_string()])
        .collect()
}

/// `(sample_index, first_engine, measurement_order)` of every record.
fn recorded_order(run: &Run) -> Vec<(u64, String, String)> {
    assert_eq!(run.records.len(), 4, "stderr:\n{}", run.stderr);
    run.records
        .iter()
        .map(|record| {
            (
                record["sample_index"].as_u64().expect("sample_index"),
                record["first_engine"]
                    .as_str()
                    .unwrap_or("<none>")
                    .to_owned(),
                record["measurement_order"]
                    .as_str()
                    .unwrap_or("<none>")
                    .to_owned(),
            )
        })
        .collect()
}

#[test]
fn default_order_runs_sqlite_first_and_records_it() {
    let run = run(None);
    assert_eq!(
        run.case_runs,
        pairs(&[("reference", "target"); 4]),
        "stderr:\n{}",
        run.stderr
    );
    for (_, first, order) in recorded_order(&run) {
        assert_eq!(
            (first.as_str(), order.as_str()),
            ("reference", "sqlite_first")
        );
    }
    let manifest = run.manifest.expect("suite manifest");
    assert_eq!(manifest["measurement_order"], "sqlite_first");
}

#[test]
fn alternate_order_swaps_engines_by_sample_index() {
    let run = run(Some("alternate"));
    assert_eq!(
        run.case_runs,
        pairs(&[
            ("reference", "target"),
            ("target", "reference"),
            ("reference", "target"),
            ("target", "reference"),
        ]),
        "stderr:\n{}",
        run.stderr
    );
    assert_eq!(
        recorded_order(&run),
        [
            (0, "reference", "alternate"),
            (1, "target", "alternate"),
            (2, "reference", "alternate"),
            (3, "target", "alternate"),
        ]
        .map(|(index, first, order)| (index, first.to_owned(), order.to_owned()))
        .to_vec()
    );
    assert_eq!(
        run.manifest.expect("suite manifest")["measurement_order"],
        "alternate"
    );
}

#[test]
fn target_first_order_runs_the_target_first() {
    let run = run(Some("target-first"));
    assert_eq!(
        run.case_runs,
        pairs(&[("target", "reference"); 4]),
        "stderr:\n{}",
        run.stderr
    );
    for (_, first, order) in recorded_order(&run) {
        assert_eq!((first.as_str(), order.as_str()), ("target", "target_first"));
    }
}

#[test]
fn unknown_order_is_refused_before_anything_runs() {
    let run = run(Some("random"));
    assert_eq!(run.code, Some(2), "stderr:\n{}", run.stderr);
    assert!(run.records.is_empty());
    assert!(run.case_runs.is_empty());
}
