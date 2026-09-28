//! The PGO and BOLT training runs, and perf_run_jsonl, run the SQLite
//! workload the way the official lane does.
//!
//! Runs scripts/perf/tests/training-fixtures.sh, which drives copies of the
//! real scripts/perf/bolt.sh and pgo.sh in a scratch Git repository against
//! stub tools and a stub runner that gates on --sqlite-known-failures as
//! the real runner does. A training run whose only failure the baseline
//! lists reaches perf2bolt and llvm-bolt; an unlisted failure stops it
//! first. pgo.sh shows the same command, and perf_run_jsonl keeps the
//! target's durability notice off the stderr the runner compares.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn training_runs_hold_the_workload_to_the_known_failures_baseline() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root");
    let output = Command::new("bash")
        .arg(root.join("scripts/perf/tests/training-fixtures.sh"))
        .current_dir(&root)
        .env_remove("REDLINE_TESTING_BIN")
        .env_remove("SQLITE_REF_BIN")
        .env_remove("PERF_KNOWN_FAILURES")
        .env_remove("CI_REDLINE_TESTING_BIN")
        .output()
        .expect("run the perf training fixture harness");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "fixture harness failed ({}):\n{stdout}\n{stderr}",
        output.status
    );
    assert!(
        stdout.contains("perf training fixtures: all passed"),
        "{stdout}\n{stderr}"
    );
}
