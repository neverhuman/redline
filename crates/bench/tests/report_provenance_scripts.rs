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
fn report_update_passes_run_provenance_or_marks_the_run_historical() {
    let run = read("scripts/just/run.sh");
    let args = shell_function(&run, "sqlite_parity_report_args");
    assert!(args.contains("--run-provenance"), "{args}");
    assert!(args.contains("--historical-run"), "{args}");
}

/// Every top-level function of a shell script, in order: `name() {` at the
/// start of a line through the next `}` line.
fn all_shell_functions(script: &str) -> String {
    let mut functions = String::new();
    let mut inside = false;
    for line in script.lines() {
        if !inside && line.ends_with("() {") && !line.starts_with([' ', '\t', '#']) {
            inside = true;
        }
        if inside {
            functions.push_str(line);
            functions.push('\n');
            if line == "}" {
                inside = false;
            }
        }
    }
    functions
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

const LATEST: &str = "benchmark-results/sqlite-parity/latest";

/// A checkout-shaped scratch directory: the runner's fresh output under
/// target/redline-testing, and the previously committed report under
/// benchmark-results/sqlite-parity/latest.
struct StagingFixture {
    dir: tempfile::TempDir,
}

impl StagingFixture {
    fn new(recorded_provenance_sha256: Option<&str>) -> Self {
        let dir = tempfile::tempdir().expect("scratch checkout");
        let root = dir.path();
        let target = root.join("target/redline-testing");
        fs::create_dir_all(&target).expect("target dir");
        let provenance = b"{\"schema_version\":\"redline-testing-run-provenance-v2\"}\n";
        fs::write(target.join("provenance.json"), provenance).expect("provenance");
        fs::write(
            target.join("sqlite_parity.raw.jsonl"),
            "{\"case_id\":\"new\"}\n",
        )
        .expect("raw");
        let recorded =
            recorded_provenance_sha256.map_or_else(|| sha256_hex(provenance), str::to_owned);
        fs::write(
            target.join("official-evidence.processed.json"),
            format!("{{\"suite_summaries\":{{\"sqlite_parity\":{{\"provenance_sha256\":\"{recorded}\"}}}}}}\n"),
        )
        .expect("processed");
        let latest = root.join(LATEST);
        fs::create_dir_all(&latest).expect("latest dir");
        for (name, text) in [
            ("raw.jsonl", "{\"case_id\":\"old\"}\n"),
            ("official-evidence.processed.json", "{\"old\":true}\n"),
            ("provenance.json", "{\"stale\":true}\n"),
            ("summary.json", "{\"old\":true}\n"),
        ] {
            fs::write(latest.join(name), text).expect("committed file");
        }
        fs::write(root.join("README.md"), "# README\n").expect("readme");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("bin dir");
        let report = bin.join("redline-testing");
        fs::write(
            &report,
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$*\" >> \"$FAKE_REPORT_LOG\"\nexit \"${FAKE_REPORT_EXIT:-0}\"\n",
        )
        .expect("fake runner");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&report, fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        Self { dir }
    }

    fn root(&self) -> &std::path::Path {
        self.dir.path()
    }

    /// The committed report directory, file by file.
    fn latest(&self) -> Vec<(String, Vec<u8>)> {
        let mut files = fs::read_dir(self.root().join(LATEST))
            .expect("latest dir")
            .map(|entry| {
                let path = entry.expect("entry").path();
                (
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    fs::read(&path).expect("read"),
                )
            })
            .collect::<Vec<_>>();
        files.sort();
        files
    }

    /// Runs the update lane's staging step with run.sh's functions and a
    /// fake runner whose `report` exits `report_exit`.
    fn stage(&self, report_exit: i32) -> (std::process::ExitStatus, String) {
        let run = read("scripts/just/run.sh");
        let script = format!(
            "set -euo pipefail\n\
             sqlite_parity_full_select=(--priorities P0 --include-quarantine)\n\
             sqlite_parity_repetitions=3\n\
             sqlite_parity_warmup=1\n\
             {}\n\
             stage_sqlite_report_official_evidence \"$PWD/bin/redline-testing\" 2026-09-28\n",
            all_shell_functions(&run)
        );
        let log = self.root().join("report.log");
        let output = Command::new("bash")
            .arg("-c")
            .arg(script)
            .current_dir(self.root())
            .env("FAKE_REPORT_LOG", &log)
            .env("FAKE_REPORT_EXIT", report_exit.to_string())
            .output()
            .expect("run the staging step");
        (
            output.status,
            format!(
                "stderr: {}\nreport calls: {}",
                String::from_utf8_lossy(&output.stderr),
                fs::read_to_string(&log).unwrap_or_default()
            ),
        )
    }
}

#[cfg(unix)]
#[test]
fn report_update_stages_only_a_run_the_report_accepts() {
    // A run bound to its evidence and accepted by the report is staged:
    // the run's own provenance byte for byte, its raw results and its
    // processed evidence, and the stale provenance.json goes.
    let fixture = StagingFixture::new(None);
    let (status, log) = fixture.stage(0);
    assert!(status.success(), "{log}");
    let root = fixture.root();
    let latest = root.join(LATEST);
    for (staged, source) in [
        ("run-provenance.json", "provenance.json"),
        ("raw.jsonl", "sqlite_parity.raw.jsonl"),
        (
            "official-evidence.processed.json",
            "official-evidence.processed.json",
        ),
    ] {
        assert_eq!(
            fs::read(latest.join(staged)).expect("staged"),
            fs::read(root.join("target/redline-testing").join(source)).expect("source"),
            "{staged}"
        );
    }
    assert!(
        !latest.join("provenance.json").exists(),
        "stale provenance.json kept"
    );
    // The report was asked about the run where the runner left it, in
    // official mode, writing nowhere under the committed report.
    assert!(
        log.contains("report --suite sqlite_parity")
            && log.contains("--input target/redline-testing/sqlite_parity.raw.jsonl")
            && log.contains("--run-provenance target/redline-testing/provenance.json")
            && !log.contains(&format!("--out-dir {LATEST}")),
        "{log}"
    );

    // Evidence that does not record this provenance file's hash, and a run
    // the report refuses (a dirty source tree, an identity mismatch, no
    // oracle stamp), both leave the committed report exactly as it was.
    for (what, recorded, report_exit) in [
        ("mismatched provenance hash", Some("0".repeat(64)), 0),
        ("run the report refuses", None, 1),
    ] {
        let fixture = StagingFixture::new(recorded.as_deref());
        let before = fixture.latest();
        let (status, log) = fixture.stage(report_exit);
        assert!(!status.success(), "{what}: staging succeeded\n{log}");
        assert_eq!(fixture.latest(), before, "{what}: latest/ changed\n{log}");
    }
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
