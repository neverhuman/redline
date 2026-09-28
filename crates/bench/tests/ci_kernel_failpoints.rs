//! The kernel-failpoints CI stage must run every failpoint-gated kernel
//! test. `ops/ci/fast.sh` runs what `ops/ci/kernel-failpoint-plan.sh`
//! prints; this test runs the plan and checks that each test file that
//! mentions `feature = "failpoints"` is selected, with every feature its
//! file is gated on as a whole. Before the plan, the stage matched only
//! whole-file `#![cfg(feature = "failpoints")]`, so the per-test gated WAL
//! crash tests in wal_pipeline.rs (also gated on `wal_pipeline`) never ran.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Features a test file is gated on as a whole (`#![cfg(feature = "x")]`).
fn file_features(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("#![cfg(feature = \"")?
                .strip_suffix("\")]")
                .map(str::to_owned)
        })
        .collect()
}

#[test]
fn every_failpoint_gated_kernel_test_file_is_planned_with_its_features() {
    let root = repository_root();
    let output = Command::new("bash")
        .arg(root.join("ops/ci/kernel-failpoint-plan.sh"))
        .arg(&root)
        .output()
        .expect("run kernel-failpoint-plan.sh");
    assert!(
        output.status.success(),
        "plan failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan = String::from_utf8(output.stdout).expect("utf8 plan");
    let runs: Vec<(BTreeSet<String>, String)> = plan
        .lines()
        .map(|line| {
            let (features, filter) = line.split_once('\t').expect("features<TAB>filter");
            (
                features.split(',').map(str::to_owned).collect(),
                filter.to_owned(),
            )
        })
        .collect();
    assert!(
        runs.iter()
            .any(|(_, filter)| filter.contains("kind(lib) & test(/^failpoints::/)")),
        "the lib's failpoints:: tests are not planned: {plan}"
    );
    let mut gated = 0;
    for entry in fs::read_dir(root.join("crates/kernel/tests")).expect("kernel tests") {
        let path = entry.expect("entry").path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("read test file");
        if !text.contains("feature = \"failpoints\"") {
            continue;
        }
        gated += 1;
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let mut needed = file_features(&text);
        needed.insert("failpoints".to_owned());
        let selector = format!("binary({name})");
        let planned = runs
            .iter()
            .any(|(features, filter)| filter.contains(&selector) && needed.is_subset(features));
        assert!(
            planned,
            "{name}.rs gates tests on failpoints but no planned run selects it with \
             features {needed:?}: {plan}"
        );
    }
    assert!(gated >= 3, "found only {gated} failpoint-gated test files");
}
