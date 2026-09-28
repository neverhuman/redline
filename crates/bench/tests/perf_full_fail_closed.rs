//! The local full-corpus perf lane fails closed (BM3-01).
//!
//! Runs scripts/perf/tests/full-fail-closed.sh, which drives the real
//! scripts/perf/full.sh against a stub runner: an interrupted child, empty
//! output with exit 0, `{}`, truncated JSONL, a missing repetition or case,
//! a duplicate sample, a tampered completion marker, a non-zero exit no
//! failure explains, an unlisted failure and a stale all.jsonl must all be
//! rejected; a complete run, and one whose only failure the baseline lists,
//! pass into their own run directories.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn full_sh_rejects_every_incomplete_or_unexplained_run() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root");
    let output = Command::new("bash")
        .arg(root.join("scripts/perf/tests/full-fail-closed.sh"))
        .current_dir(&root)
        .env("PERF_EVIDENCE_BIN", env!("CARGO_BIN_EXE_perf_evidence"))
        .env_remove("REDLINE_TESTING_BIN")
        .env_remove("SQLITE_REF_BIN")
        .env_remove("PERF_ROOT")
        .env_remove("PERF_TASKSET_CPUS")
        .output()
        .expect("run the full.sh fixture harness");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "fixture harness failed ({}):\n{stdout}\n{stderr}",
        output.status
    );
    assert!(
        stdout.contains("full.sh fail-closed fixtures: all passed"),
        "{stdout}\n{stderr}"
    );
}
