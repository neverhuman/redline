//! The release bench bundle script (L-05) measures only what it claims.
//!
//! Runs scripts/perf/tests/release-bench-fixtures.sh, which drives the real
//! scripts/perf/release-bench.sh against a stub runner, and the argument
//! checks of scripts/perf/build-version.sh. A complete interleaved bundle
//! is written and summarized. A failpoints or debug build, a mislabelled
//! build record, a reused bundle name, an overloaded host, an incomplete
//! run, an unexplained runner exit and an unknown case-list id are refused.
//! A failed case is kept as data, a case list narrows the runs, and
//! --durability default exports no override.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn release_bench_writes_only_complete_accepted_bundles() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root");
    let output = Command::new("bash")
        .arg(root.join("scripts/perf/tests/release-bench-fixtures.sh"))
        .current_dir(&root)
        .env("PERF_EVIDENCE_BIN", env!("CARGO_BIN_EXE_perf_evidence"))
        .env_remove("REDLINE_TESTING_BIN")
        .env_remove("SQLITE_REF_BIN")
        .env_remove("VERSION_BUILD_RUSTFLAGS")
        .output()
        .expect("run the release-bench fixture harness");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "fixture harness failed ({}):\n{stdout}\n{stderr}",
        output.status
    );
    assert!(
        stdout.contains("release-bench fixtures: all passed"),
        "{stdout}\n{stderr}"
    );
}
