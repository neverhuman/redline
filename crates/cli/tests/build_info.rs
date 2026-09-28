//! `redlinedb --build-info [--json]` names the build a binary came from:
//! its package version, the release tag and source commit it was packaged
//! from (`REDLINEDB_BUILD_TAG`/`REDLINEDB_BUILD_SHA` at compile time, which
//! `scripts/package-release.sh` sets), its target, the one repository
//! allowed to release it, and the SQLite version the parity oracle runs.
//! The release workflow's `verify-published` job compares the published
//! binary's JSON against the tag and commit it released.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

fn repo_root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn build_info(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_redlinedb"))
        .args(args)
        .output()
        .expect("run redlinedb");
    assert!(
        output.status.success(),
        "redlinedb {args:?} failed: status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf8 stdout")
}

fn build_info_json(args: &[&str]) -> Value {
    let stdout = build_info(args);
    assert_eq!(
        stdout.lines().count(),
        1,
        "--build-info --json prints one line: {stdout}"
    );
    serde_json::from_str(&stdout).unwrap_or_else(|err| panic!("{err}: {stdout}"))
}

/// `KEY=value` from a shell assignment file such as ops/release/authority.env.
fn assignment(path: &str, key: &str) -> String {
    let text = std::fs::read_to_string(repo_root().join(path)).expect(path);
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("{path} lacks {key}="))
        .trim_matches('"')
        .to_owned()
}

#[test]
fn json_names_version_source_target_and_release_authority() {
    let info = build_info_json(&["--build-info", "--json"]);
    assert_eq!(info["schema"], "redline.build-info/v1", "{info}");
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"), "{info}");
    assert_eq!(
        info["source_sha"],
        option_env!("REDLINEDB_BUILD_SHA").unwrap_or("unknown"),
        "{info}"
    );
    match option_env!("REDLINEDB_BUILD_TAG") {
        Some(tag) => assert_eq!(info["tag"], tag, "{info}"),
        None => assert!(info["tag"].is_null(), "{info}"),
    }
    assert_eq!(
        info["target"],
        format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        "{info}"
    );
    // The release authority is ops/release/authority.env; the binary must not
    // drift from it.
    assert_eq!(
        info["repository_url"],
        assignment("ops/release/authority.env", "REDLINE_REPO_URL"),
        "{info}"
    );
    let id: u64 = assignment("ops/release/authority.env", "REDLINE_REPO_ID")
        .parse()
        .expect("numeric REDLINE_REPO_ID");
    assert_eq!(info["repository_id"], id, "{info}");
    assert_eq!(info["repository_id"], 1_390_165_945_u64, "{info}");
    // The oracle is the SQLite that scripts/sqlite/build-reference.sh pins.
    assert_eq!(
        info["sqlite_oracle"],
        assignment("scripts/sqlite/build-reference.sh", "version"),
        "{info}"
    );
}

#[test]
fn single_dash_spelling_matches_double_dash() {
    assert_eq!(
        build_info_json(&["-build-info", "-json"]),
        build_info_json(&["--build-info", "--json"])
    );
}

#[test]
fn build_info_ignores_a_database_argument_and_touches_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let database = dir.path().join("never-created.db");
    let output = Command::new(env!("CARGO_BIN_EXE_redlinedb"))
        .current_dir(dir.path())
        .args(["--build-info", "--json"])
        .arg(&database)
        .output()
        .expect("run redlinedb");
    assert!(output.status.success(), "{output:?}");
    assert!(!database.exists(), "--build-info opened {database:?}");
    assert_eq!(
        std::fs::read_dir(dir.path()).expect("read dir").count(),
        0,
        "--build-info wrote into the working directory"
    );
}

#[test]
fn text_form_has_one_labelled_field_per_line() {
    let json = build_info_json(&["--build-info", "--json"]);
    let text = build_info(&["--build-info"]);
    let tag = json["tag"].as_str().unwrap_or("none");
    let expected = format!(
        "redlinedb {version}\n\
         tag: {tag}\n\
         source: {sha}\n\
         target: {target}\n\
         repository: {url} (id {id})\n\
         sqlite oracle: {oracle}\n",
        version = json["version"].as_str().unwrap(),
        sha = json["source_sha"].as_str().unwrap(),
        target = json["target"].as_str().unwrap(),
        url = json["repository_url"].as_str().unwrap(),
        id = json["repository_id"],
        oracle = json["sqlite_oracle"].as_str().unwrap(),
    );
    assert_eq!(text, expected);
}
