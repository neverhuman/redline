//! Black-box `redline-testing report` provenance tests (SQ-03/L-03/R4-03).
//!
//! These run the real binary with a hostile environment: a different
//! `sqlite3` first on PATH, bogus `REDLINE_TESTING_*_BIN` overrides, and
//! different Git states between rendering and checking. The published
//! measurement identity must come from the run evidence alone.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const TARGET_SHA256: &str = "be49779bace1d97c1d9a78f697f3a7b2bd27c6c4c6a5dccefd29304dfaa55b78";
const SQLITE_SHA256: &str = "e99d817b62f1ad9ead02b8d4e410fea9d736ee82c35b9a7daa6644d4d3e5ae3a";
const RUNNER_SHA256: &str = "b28c41d40009bfe7c98832abda695c3b9d2624871c4c886991601718d8e70a78";
const SQLITE_VERSION: &str = "3.53.1 2026-05-05 10:34:17 c88b22011a54 (64-bit)";
const TARGET_VERSION: &str = "redlinedb v4.1.0 (SQLite 3.45.1 compatibility)";
const PATH_SQLITE_VERSION: &str = "3.45.1 2024-01-30 16:01:20 path-sqlite (64-bit)";
const README: &str = "# Report\n\nIntro.\n\n<!-- sqlite-parity-report:begin -->\n<!-- sqlite-parity-report:end -->\n\nOutro.\n";

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The ids of every sqlite_parity case, from the runner's own listing: an
/// official report must cover exactly them (SQ-04).
fn corpus_case_ids() -> Vec<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_redline-testing"))
        .args(["list", "--suite", "sqlite_parity", "--format", "json"])
        .output()
        .expect("list the corpus");
    assert!(output.status.success(), "list failed");
    let cases: Value = serde_json::from_slice(&output.stdout).expect("case list");
    cases
        .as_array()
        .expect("case array")
        .iter()
        .map(|case| format!("{:05}", case["id"].as_u64().expect("case id")))
        .collect()
}

/// Every corpus case, passed and measured three times.
fn raw_text() -> String {
    let mut raw = String::new();
    for case_id in corpus_case_ids() {
        for repetition in 1..=3usize {
            let record = json!({
                "case_id": case_id,
                "name": format!("CASE_{case_id}"),
                "case_file": format!("CASE_{case_id}.rs"),
                "priority": "P0",
                "profile": "memory",
                "category": "SMOKE",
                "sample_role": format!("measured:{repetition}"),
                "repetition_index": repetition,
                "status": "passed",
                "reference_executable_path": "/ci/sqlite-reference/3.53.1/bin/sqlite3",
                "target_executable_path": "/ci/release/redlinedb",
                "reference_executable_sha256": SQLITE_SHA256,
                "target_executable_sha256": TARGET_SHA256,
                "reference_version": SQLITE_VERSION,
                "target_version": TARGET_VERSION,
                "reference_elapsed_ns": 2_000_000u64,
                "target_elapsed_ns": 4_000_000u64,
            });
            raw.push_str(&format!("{record}\n"));
        }
    }
    raw
}

/// A run directory: raw results, run provenance and processed evidence.
struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("redline-testing-report-cli-")
            .tempdir()
            .expect("temp root");
        let raw = raw_text();
        let cases = raw.lines().count() / 3;
        let identity = json!({
            "source_commit": "c1af369c1af369c1af369c1af369c1af369c1af3",
            "source_tree": "7ee7ee7ee7ee7ee7ee7ee7ee7ee7ee7ee7ee7ee7",
            "source_inputs_sha256": "cf704799b75ce7408d3ce1adec3cf7e93aa1ca9f25a6dbbce1429782bf34e22f",
            "source_dirty": false,
            "corpus_sha256": "c0ffeec0ffeec0ffeec0ffeec0ffeec0ffeec0ffeec0ffeec0ffeec0ffeec0ff",
            "assertion_policy_sha256": "a55e47a55e47a55e47a55e47a55e47a55e47a55e47a55e47a55e47a55e47a55e",
            "oracle_build_stamp": "36ca143645cf76997d07b66e9244c636b8ccdec64a1d50558259c4e415e6558b\n-O2",
        });
        let mut run_provenance = json!({
            "schema_version": "redline-testing-run-provenance-v2",
            "suite": "sqlite_parity",
            "target_binary_path": "/ci/release/redlinedb",
            "target_binary_sha256": TARGET_SHA256,
            "target_version": TARGET_VERSION,
            "redline_testing_binary_path": "/ci/release/redline-testing",
            "redline_testing_binary_sha256": RUNNER_SHA256,
            "redline_testing_version": "redline-testing 1.0.1",
            "sqlite_binary_path": "/ci/sqlite-reference/3.53.1/bin/sqlite3",
            "sqlite_binary_sha256": SQLITE_SHA256,
            "sqlite_version": SQLITE_VERSION,
            "source_dirty_paths": [],
            "elapsed_ns": 1_395_000_000u64,
            "output_file_hashes": { "sqlite_parity.raw.jsonl": sha256_hex(raw.as_bytes()) },
        });
        let mut official = json!({
            "schema_version": "redline-testing-official-evidence-v1",
            "run_provenance_schema": "redline-testing-run-provenance-v2",
            "runner": {
                "binary_path": "/ci/release/redline-testing",
                "binary_sha256": RUNNER_SHA256,
                "version": "redline-testing 1.0.1",
            },
            "target": { "path": "/ci/release/redlinedb", "sha256": TARGET_SHA256, "version": TARGET_VERSION },
            "sqlite": {
                "path": "/ci/sqlite-reference/3.53.1/bin/sqlite3",
                "sha256": SQLITE_SHA256,
                "version": SQLITE_VERSION,
            },
            "status": "passed",
            "command_line": ["/ci/release/redline-testing", "run", "--suite", "all", "--workers", "auto"],
            "workers": "128",
        });
        for (key, value) in identity.as_object().expect("identity") {
            run_provenance[key] = value.clone();
            official[key] = value.clone();
        }
        let run_provenance_text = serde_json::to_string_pretty(&run_provenance).unwrap() + "\n";
        let evidence = json!({
            "schema_version": "redline-testing-official-evidence-processed-v1",
            "source_sha256": "daac7524c76944c99fdaf6ac397034c9aa5634906f035771ba83d7fd10e54ccc",
            "status": "passed",
            "official_evidence": official,
            "suite_summaries": {
                "sqlite_parity": {
                    "total": cases, "passed": cases, "failed": 0, "skipped": 0,
                    "raw_path": "sqlite_parity.raw.jsonl",
                    "provenance_path": "provenance.json",
                    "raw_sha256": sha256_hex(raw.as_bytes()),
                    "provenance_sha256": sha256_hex(run_provenance_text.as_bytes()),
                    // The finished run's completion marker, as the
                    // evidence processor copies it.
                    "completion": {
                        "schema_version": "redline-testing-raw-complete-v1",
                        "suite": "sqlite_parity",
                        "raw_file": "sqlite_parity.raw.jsonl",
                        "records": raw.lines().count(),
                        "cases": cases,
                        "raw_sha256": sha256_hex(raw.as_bytes()),
                    },
                }
            },
        });
        let run = root.path().join("run");
        fs::create_dir_all(&run).expect("run dir");
        fs::write(run.join("raw.jsonl"), &raw).expect("raw");
        fs::write(run.join("run-provenance.json"), run_provenance_text).expect("run provenance");
        fs::write(
            run.join("official-evidence.processed.json"),
            serde_json::to_string_pretty(&evidence).unwrap() + "\n",
        )
        .expect("evidence");
        fs::write(run.join("README.md"), README).expect("readme");
        Self { root }
    }

    fn run_dir(&self) -> PathBuf {
        self.root.path().join("run")
    }

    fn out_dir(&self) -> PathBuf {
        self.run_dir().join("out")
    }

    /// `report` arguments; every path is absolute, so the working
    /// directory can be anything.
    fn report_args(&self) -> Vec<String> {
        let run = self.run_dir();
        let path = |name: &str| run.join(name).display().to_string();
        vec![
            "report".into(),
            "--suite".into(),
            "sqlite_parity".into(),
            "--input".into(),
            path("raw.jsonl"),
            "--official-evidence".into(),
            path("official-evidence.processed.json"),
            "--run-provenance".into(),
            path("run-provenance.json"),
            "--out-dir".into(),
            self.out_dir().display().to_string(),
            "--readme".into(),
            path("README.md"),
            "--updated-date".into(),
            "2026-09-24".into(),
            "--expected-repetitions".into(),
            "3".into(),
            "--expected-warmup".into(),
            "0".into(),
        ]
    }

    /// A bin directory whose `sqlite3` is a different SQLite release.
    fn path_sqlite_dir(&self) -> PathBuf {
        let dir = self.root.path().join("path-bin");
        fs::create_dir_all(&dir).expect("path bin dir");
        let sqlite = dir.join("sqlite3");
        fs::write(
            &sqlite,
            format!("#!/bin/sh\nprintf '%s\\n' '{PATH_SQLITE_VERSION}'\n"),
        )
        .expect("fake sqlite3");
        make_executable(&sqlite);
        dir
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod");
}

fn report(args: &[String], cwd: &Path, path_env: &str, envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_redline-testing"));
    command
        .args(args)
        .current_dir(cwd)
        .env("PATH", path_env)
        .env_remove("REDLINE_TESTING_SQLITE_BIN")
        .env_remove("REDLINE_TESTING_TARGET_BIN");
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("run redline-testing report")
}

fn assert_success(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Every JSON file the report wrote, by file name.
fn json_outputs(out_dir: &Path) -> Vec<(String, String)> {
    let mut outputs = fs::read_dir(out_dir)
        .expect("out dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                fs::read_to_string(&path).expect("json output"),
            )
        })
        .collect::<Vec<_>>();
    outputs.sort();
    outputs
}

#[cfg(unix)]
#[test]
fn report_ignores_path_sqlite_uses_run_identity() {
    let fixture = Fixture::new();
    let path_env = format!(
        "{}:{}",
        fixture.path_sqlite_dir().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let args = fixture.report_args();
    let cwd = fixture.root.path();

    // A different sqlite3 first on PATH, no override.
    assert_success(
        &report(&args, cwd, &path_env, &[]),
        "report with PATH sqlite3",
    );
    let first = json_outputs(&fixture.out_dir());
    // Overrides that name no executable at all.
    fs::remove_dir_all(fixture.out_dir()).expect("clear out dir");
    let bogus = [
        ("REDLINE_TESTING_SQLITE_BIN", "/nonexistent/sqlite3"),
        ("REDLINE_TESTING_TARGET_BIN", "/nonexistent/redlinedb"),
    ];
    assert_success(
        &report(&args, cwd, &path_env, &bogus),
        "report with bogus overrides",
    );
    let second = json_outputs(&fixture.out_dir());

    assert!(!first.is_empty(), "report wrote no JSON outputs");
    for (name, text) in &first {
        for probe in ["3.45.1 2024-01-30", "<unknown>", "/nonexistent", "path-bin"] {
            assert!(!text.contains(probe), "{name} records {probe:?}:\n{text}");
        }
    }
    assert_eq!(first, second, "rendering-host overrides changed the report");
    let joined = first
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<String>();
    assert!(
        joined.contains(SQLITE_SHA256),
        "no measured oracle digest:\n{joined}"
    );
    assert!(
        joined.contains(TARGET_SHA256),
        "no measured target digest:\n{joined}"
    );

    let provenance: Value = serde_json::from_str(
        &fs::read_to_string(fixture.out_dir().join("report-provenance.json"))
            .expect("report-provenance.json"),
    )
    .expect("report provenance json");
    let sqlite = &provenance["measurement"]["sqlite"];
    assert_eq!(sqlite["sha256"], SQLITE_SHA256);
    assert_eq!(sqlite["version"], SQLITE_VERSION);
    assert_eq!(sqlite["path"], "/ci/sqlite-reference/3.53.1/bin/sqlite3");
    assert_eq!(provenance["measurement"]["target"]["sha256"], TARGET_SHA256);
    assert_eq!(provenance["mode"], "official");
    let run_provenance = fs::read(fixture.run_dir().join("run-provenance.json")).expect("run");
    assert_eq!(
        provenance["parent_run_provenance"]["sha256"],
        sha256_hex(&run_provenance)
    );
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=report-test",
            "-c",
            "user.email=report-test@example.invalid",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

#[cfg(unix)]
#[test]
fn report_check_independent_of_git_state() {
    let fixture = Fixture::new();
    let path_env = std::env::var("PATH").unwrap_or_default();
    let args = fixture.report_args();

    // Render inside a dirty Git checkout with a commit.
    let dirty = fixture.root.path().join("dirty-checkout");
    fs::create_dir_all(&dirty).expect("dirty checkout");
    git(&dirty, &["init", "--quiet"]);
    fs::write(dirty.join("tracked.txt"), "one\n").expect("tracked");
    git(&dirty, &["add", "tracked.txt"]);
    git(&dirty, &["commit", "--quiet", "-m", "one"]);
    fs::write(dirty.join("tracked.txt"), "two\n").expect("dirty edit");
    fs::write(dirty.join("untracked.txt"), "new\n").expect("untracked");
    assert_success(
        &report(&args, &dirty, &path_env, &[]),
        "report in a dirty checkout",
    );

    // Check from outside any Git checkout, with no git on PATH at all.
    let plain = fixture.root.path().join("plain");
    fs::create_dir_all(&plain).expect("plain dir");
    let mut check = args.clone();
    check.push("--check".into());
    assert_success(
        &report(&check, &plain, "/nonexistent-bin", &[]),
        "report --check outside Git",
    );

    // README text outside the generated blocks is not report output.
    let readme = fixture.run_dir().join("README.md");
    let edited = fs::read_to_string(&readme)
        .expect("readme")
        .replace("Intro.", "Intro, reworded.");
    fs::write(&readme, edited).expect("edit readme");
    assert_success(
        &report(&check, &plain, "/nonexistent-bin", &[]),
        "report --check after a README edit outside the blocks",
    );
    for (name, text) in json_outputs(&fixture.out_dir()) {
        assert!(!text.contains("git_"), "{name} records Git state:\n{text}");
    }
}

fn assert_drift(output: &Output, what: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() && stderr.contains("artifact drift detected"),
        "{what}: --check must fail on drift ({}):\nstderr: {stderr}",
        output.status
    );
}

/// `--check` keeps only the committed `renderer` block (whichever build
/// wrote the files); every other byte it compares must still be the
/// rendered one.
#[cfg(unix)]
#[test]
fn report_check_fails_on_drift_outside_the_renderer_block() {
    let fixture = Fixture::new();
    let path_env = std::env::var("PATH").unwrap_or_default();
    let args = fixture.report_args();
    let cwd = fixture.root.path();
    assert_success(&report(&args, cwd, &path_env, &[]), "render");
    let mut check = args.clone();
    check.push("--check".into());
    assert_success(&report(&check, cwd, &path_env, &[]), "clean --check");

    let out = fixture.out_dir();
    let readme = fixture.run_dir().join("README.md");
    let provenance_path = out.join("report-provenance.json");
    let summary_path = out.join("summary.json");
    // Text edits, so that nothing but the named value changes.
    let replace_once = |path: &Path, from: &str, to: &str| {
        let text = fs::read_to_string(path).expect("read artifact");
        assert_eq!(
            text.matches(from).count(),
            1,
            "{from:?} in {}",
            path.display()
        );
        fs::write(path, text.replace(from, to)).expect("write artifact");
    };
    let provenance: Value =
        serde_json::from_str(&fs::read_to_string(&provenance_path).expect("provenance"))
            .expect("provenance json");
    let parent_sha = provenance["parent_run_provenance"]["sha256"]
        .as_str()
        .expect("parent sha")
        .to_owned();
    let renderer_sha = provenance["renderer"]["binary_sha256"]
        .as_str()
        .expect("renderer sha")
        .to_owned();
    let renderer_version = provenance["renderer"]["version"]
        .as_str()
        .expect("renderer version")
        .to_owned();
    type Mutation<'a> = (&'a str, &'a Path, Box<dyn Fn(&Path) + 'a>);
    let mutations: Vec<Mutation> = vec![
        (
            "measurement.sqlite.version",
            &provenance_path,
            Box::new(|path| {
                replace_once(
                    path,
                    &format!("\"version\": \"{SQLITE_VERSION}\""),
                    &format!("\"version\": \"{PATH_SQLITE_VERSION}\""),
                );
            }),
        ),
        (
            "parent_run_provenance.sha256",
            &provenance_path,
            Box::new(|path| replace_once(path, &parent_sha, &"0".repeat(64))),
        ),
        (
            "one byte inside the README report block",
            &readme,
            Box::new(|path| {
                let text = fs::read_to_string(path).expect("readme");
                let begin = text
                    .find("<!-- sqlite-parity-report:begin -->")
                    .expect("report block");
                let at = begin + text[begin..].find("**").expect("bold text in block");
                let mut bytes = text.into_bytes();
                bytes[at] = b'_';
                fs::write(path, bytes).expect("write readme");
            }),
        ),
        (
            "summary.json elapsed_ns",
            &summary_path,
            Box::new(|path| {
                replace_once(path, "\"elapsed_ns\": 1395000000", "\"elapsed_ns\": 1");
            }),
        ),
    ];
    for (what, path, mutate) in mutations {
        let original = fs::read(path).expect("read artifact");
        mutate(path);
        assert_ne!(
            fs::read(path).expect("reread"),
            original,
            "{what} unchanged"
        );
        assert_drift(&report(&check, cwd, &path_env, &[]), what);
        fs::write(path, &original).expect("restore artifact");
        assert_success(
            &report(&check, cwd, &path_env, &[]),
            &format!("--check after restoring {what}"),
        );
    }

    // The renderer block alone may name another build.
    replace_once(&provenance_path, &renderer_sha, &"f".repeat(64));
    let text = fs::read_to_string(&provenance_path).expect("provenance");
    let block = text.find("\"renderer\": {").expect("renderer block");
    let version = format!("\"version\": \"{renderer_version}\"");
    let at = block + text[block..].find(&version).expect("renderer version");
    let edited = format!(
        "{}\"version\": \"redline-testing 0.0.0-other-build\"{}",
        &text[..at],
        &text[at + version.len()..]
    );
    fs::write(&provenance_path, edited).expect("write provenance");
    assert_success(
        &report(&check, cwd, &path_env, &[]),
        "--check with another build's renderer block",
    );
}
