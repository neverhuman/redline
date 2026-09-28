//! A run's source identity is the source it started from (SQ-03/L-03).
//!
//! The runner builds nothing, so the binaries it measures were built from
//! the checkout as it stood when the run started. A commit made in the same
//! checkout while the run is going (another agent, an editor, a hook) must
//! not be credited with those binaries: the evidence must name the start
//! commit, or the run must refuse to write evidence at all.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

fn sqlite3() -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH is set");
    std::env::split_paths(&path)
        .map(|dir| dir.join("sqlite3"))
        .find(|candidate| candidate.is_file())
        .expect("sqlite3 must be on PATH (the reference of these runs)")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Answers `--version` as RedlineDB and fails the capability probes; on
/// its first case run it commits a source change to the checkout the run
/// started from, then answers the case exactly as the reference does.
const COMMITTING_TARGET: &str = r#"#!/usr/bin/env bash
case "$1" in
  --version) echo "redlinedb 0.0.0-run-identity"; exit 0 ;;
  --batch) ;;
  *) cat >/dev/null; exit 1 ;;
esac
if [ ! -e "$FAKE_REPO/.committed" ]; then
  : > "$FAKE_REPO/.committed"
  echo "fn changed() {}" > "$FAKE_REPO/crates/engine/lib.rs"
  git -C "$FAKE_REPO" -c user.name=t -c user.email=t@example.invalid \
    -c core.hooksPath=/dev/null -c commit.gpgsign=false commit --quiet -am "mid-run" >/dev/null
fi
exec "$FAKE_SQLITE" -batch -bail "$3"
"#;

#[test]
fn a_commit_made_during_the_run_is_not_credited_with_its_binaries() {
    let root = tempfile::Builder::new()
        .prefix("redline-testing-run-identity-")
        .tempdir()
        .expect("temp checkout");
    let repo = root.path().join("checkout");
    fs::create_dir_all(repo.join("crates/engine")).expect("crates dir");
    fs::write(repo.join("crates/engine/lib.rs"), "fn original() {}\n").expect("source");
    fs::write(repo.join(".gitignore"), "out/\n.committed\n").expect("gitignore");
    git(&repo, &["init", "--quiet"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "start"]);
    let start = git(&repo, &["rev-parse", "HEAD"]);

    let target = root.path().join("fake-redlinedb");
    fs::write(&target, COMMITTING_TARGET).expect("fake target");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).expect("chmod");
    let out = repo.join("out");
    let raw = out.join("sqlite_parity.raw.jsonl");
    let output = Command::new(env!("CARGO_BIN_EXE_redline-testing"))
        .current_dir(&repo)
        .args(["run", "--suite", "sqlite_parity", "--case-id", "00001"])
        .arg("--target-bin")
        .arg(&target)
        .arg("--sqlite-bin")
        .arg(sqlite3())
        .args(["--workers", "1", "--repetitions", "1", "--warmup", "0"])
        .args(["--progress", "never", "--tmp-root"])
        .arg(root.path().join("tmp"))
        .arg("--output")
        .arg(&raw)
        .env("FAKE_REPO", &repo)
        .env("FAKE_SQLITE", sqlite3())
        .stdin(Stdio::null())
        .output()
        .expect("run redline-testing");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let end = git(&repo, &["rev-parse", "HEAD"]);
    assert_ne!(
        start, end,
        "the fake target did not commit; stderr:\n{stderr}"
    );

    // Evidence, if any, names the commit the run started from.
    if let Ok(text) = fs::read_to_string(out.join("provenance.json")) {
        let provenance: Value = serde_json::from_str(&text).expect("provenance json");
        assert_eq!(
            provenance["source_commit"], start,
            "the run credited its binaries to the commit made during it:\n{text}"
        );
    }
    // And the run refuses to certify a source that moved under it.
    assert!(
        !output.status.success() && stderr.contains("changed during the run"),
        "the run accepted a source that changed under it ({}):\n{stderr}",
        output.status
    );
}
