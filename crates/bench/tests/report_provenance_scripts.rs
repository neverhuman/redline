//! Report and perf wrapper provenance (SQ-03/L-03/R4-03): the report check
//! must not rewrite provenance to match the local checkout, the report
//! update must stage the run's own provenance, and perf scripts must use
//! the runner and reference built in this checkout.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn read(path: &str) -> String {
    let path = repository_root().join(path);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// The body of a top-level shell function, up to its closing brace.
fn shell_function<'a>(script: &'a str, name: &str) -> &'a str {
    let start = script
        .find(&format!("\n{name}() {{\n"))
        .unwrap_or_else(|| panic!("{name}() not found"));
    let body = &script[start + 1..];
    let end = body
        .find("\n}\n")
        .unwrap_or_else(|| panic!("{name}() never closes"));
    &body[..end]
}

#[test]
fn report_check_does_not_rewrite_provenance() {
    let run = read("scripts/just/run.sh");
    let check = shell_function(&run, "run_sqlite_parity_report_check");
    for forbidden in [
        "jq",
        "git rev-parse",
        "git_dirty",
        "git_sha",
        "sha256sum README.md",
    ] {
        assert!(
            !check.contains(forbidden),
            "run_sqlite_parity_report_check rewrites provenance with {forbidden:?}:\n{check}"
        );
    }
    assert!(check.contains("--check"), "{check}");
}

#[test]
fn report_update_stages_the_bound_run_provenance() {
    let run = read("scripts/just/run.sh");
    let stage = shell_function(&run, "stage_sqlite_report_official_evidence");
    assert!(
        stage.contains("run-provenance.json"),
        "the run's provenance is not staged:\n{stage}"
    );
    assert!(
        stage.contains("suite_summaries.sqlite_parity.provenance_sha256"),
        "the staged run provenance is not checked against the evidence:\n{stage}"
    );
    let args = shell_function(&run, "sqlite_parity_report_args");
    assert!(args.contains("--run-provenance"), "{args}");
    assert!(args.contains("--historical-run"), "{args}");
}

#[test]
fn perf_scripts_default_to_the_in_tree_runner() {
    let root = repository_root();
    let output = Command::new("bash")
        .arg("-c")
        .arg(". scripts/perf/lib.sh && printf '%s\\n%s\\n' \"$REDLINE_TESTING_BIN\" \"$SQLITE_REF_BIN\"")
        .current_dir(&root)
        .env_remove("REDLINE_TESTING_BIN")
        .env_remove("SQLITE_REF_BIN")
        .env_remove("SQLITE_REF_BIN_DEFAULT")
        .env_remove("CI_REDLINE_TESTING_BIN")
        .env_remove("REDLINE_CORE_ROOT")
        .env_remove("REDLINE_SPLIT_ROOT")
        .output()
        .expect("source scripts/perf/lib.sh");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    assert_eq!(
        lines.next().map(PathBuf::from),
        Some(root.join("target/release/redline-testing"))
    );
    assert_eq!(
        lines.next().map(PathBuf::from),
        Some(root.join("target/sqlite-reference/3.53.1/bin/sqlite3"))
    );
    for script in ["scripts/perf/pgo.sh", "scripts/perf/bolt.sh"] {
        let text = read(script);
        assert!(
            !text.contains("REDLINE_SPLIT_ROOT") && !text.contains("sibling"),
            "{script} still defaults to a sibling checkout"
        );
        assert!(
            text.contains("target/release/redline-testing"),
            "{script} does not default to the in-tree runner"
        );
    }
}
