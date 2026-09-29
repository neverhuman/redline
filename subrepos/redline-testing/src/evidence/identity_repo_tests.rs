//! Source and run identity taken in scratch Git repositories: what makes a
//! run dirty, what counts as a changed start, and a tree outside Git.

use std::fs;
use std::path::Path;
use std::process::Command;

use super::{capture_in, ensure_unchanged, oracle_build_stamp, source_identity};

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
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
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

#[test]
fn dirtiness_counts_untracked_inputs_but_not_generated_outputs() {
    let root = tempfile::Builder::new()
        .prefix("redline-testing-source-identity-")
        .tempdir()
        .expect("temp repo");
    let repo = root.path();
    git(repo, &["init", "--quiet"]);
    fs::create_dir_all(repo.join("crates/engine")).expect("crates dir");
    fs::write(repo.join("crates/engine/lib.rs"), "fn a() {}\n").expect("source");
    fs::write(repo.join("README.md"), "# readme\n").expect("readme");
    git(repo, &["add", "."]);
    git(repo, &["commit", "--quiet", "-m", "base"]);

    let clean = source_identity(repo);
    assert_eq!(clean.source_dirty, Some(false));
    assert!(clean.source_inputs_sha256.is_some());

    // Generated outputs and ignored build output are not source inputs.
    fs::write(repo.join("README.md"), "# regenerated\n").expect("readme edit");
    fs::create_dir_all(repo.join("benchmark-results")).expect("results dir");
    fs::write(repo.join("benchmark-results/raw.jsonl"), "{}\n").expect("results");
    let outputs_only = source_identity(repo);
    assert_eq!(outputs_only.source_dirty, Some(false));
    assert_eq!(
        outputs_only.source_inputs_sha256,
        clean.source_inputs_sha256
    );

    // An untracked file under an input path makes the run dirty.
    fs::write(repo.join("crates/engine/extra.rs"), "fn b() {}\n").expect("untracked");
    let dirty = source_identity(repo);
    assert_eq!(dirty.source_dirty, Some(true));
    assert_eq!(dirty.source_dirty_paths, ["?? crates/engine/extra.rs"]);
    assert_eq!(dirty.source_tree, clean.source_tree);
}

#[test]
fn cargo_configuration_is_a_source_input() {
    // Cargo reads .cargo/config.toml from the repository root for both
    // measured builds (redlinedb-cli and the runner), so an uncommitted
    // codegen flag there changes the binaries the run measures.
    let root = tempfile::Builder::new()
        .prefix("redline-testing-cargo-config-")
        .tempdir()
        .expect("temp repo");
    let repo = root.path();
    git(repo, &["init", "--quiet"]);
    fs::create_dir_all(repo.join(".cargo")).expect("cargo dir");
    fs::write(repo.join(".cargo/config.toml"), "[build]\n").expect("config");
    git(repo, &["add", "."]);
    git(repo, &["commit", "--quiet", "-m", "base"]);
    let clean = source_identity(repo);
    assert_eq!(clean.source_dirty, Some(false));

    fs::write(
        repo.join(".cargo/config.toml"),
        "[build]\nrustflags = [\"-C\", \"target-cpu=native\"]\n",
    )
    .expect("edit config");
    let dirty = source_identity(repo);
    assert_eq!(
        dirty.source_dirty,
        Some(true),
        "{:?}",
        dirty.source_dirty_paths
    );
    assert_eq!(dirty.source_dirty_paths, [" M .cargo/config.toml"]);

    // And a committed change moves the inputs hash.
    git(repo, &["commit", "--quiet", "-am", "native"]);
    let committed = source_identity(repo);
    assert_ne!(committed.source_inputs_sha256, clean.source_inputs_sha256);
}

#[test]
fn a_commit_after_the_start_is_a_changed_run_identity() {
    let root = tempfile::Builder::new()
        .prefix("redline-testing-identity-change-")
        .tempdir()
        .expect("temp repo");
    let repo = root.path();
    git(repo, &["init", "--quiet"]);
    fs::create_dir_all(repo.join("crates")).expect("crates dir");
    fs::write(repo.join("crates/lib.rs"), "fn a() {}\n").expect("source");
    git(repo, &["add", "."]);
    git(repo, &["commit", "--quiet", "-m", "start"]);
    let sqlite = repo.join("sqlite3");
    let start = capture_in(repo, &sqlite);
    ensure_unchanged(&start, &capture_in(repo, &sqlite)).expect("nothing changed");
    // An uncommitted edit is the run's dirtiness, not a changed start.
    fs::write(repo.join("crates/lib.rs"), "fn b() {}\n").expect("edit");
    ensure_unchanged(&start, &capture_in(repo, &sqlite)).expect("still the same commit");
    git(repo, &["commit", "--quiet", "-am", "during the run"]);
    let error = ensure_unchanged(&start, &capture_in(repo, &sqlite))
        .expect_err("a new commit is a different source");
    assert!(
        format!("{error:#}").contains("the source commit changed during the run"),
        "{error:#}"
    );
}

#[test]
fn source_identity_outside_git_is_unknown_not_clean() {
    let root = tempfile::Builder::new()
        .prefix("redline-testing-no-git-")
        .tempdir()
        .expect("temp dir");
    let identity = source_identity(root.path());
    assert_eq!(identity.source_dirty, None);
    assert_eq!(identity.source_commit, None);
    assert_eq!(identity.source_inputs_sha256, None);
}

#[cfg(unix)]
#[test]
fn oracle_build_stamp_reads_the_reference_prefix() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::Builder::new()
        .prefix("redline-testing-oracle-stamp-")
        .tempdir()
        .expect("temp prefix");
    let bin = root.path().join("bin");
    fs::create_dir_all(&bin).expect("bin dir");
    let sqlite = bin.join("sqlite3");
    fs::write(&sqlite, "#!/bin/sh\n").expect("sqlite3");
    fs::set_permissions(&sqlite, fs::Permissions::from_mode(0o755)).expect("chmod");
    assert_eq!(oracle_build_stamp(&sqlite), None);
    fs::write(
        root.path().join(".sqlite-reference-sha3"),
        "36ca1436\n-O2 -DSQLITE_ENABLE_FTS5\n",
    )
    .expect("stamp");
    assert_eq!(
        oracle_build_stamp(&sqlite).as_deref(),
        Some("36ca1436\n-O2 -DSQLITE_ENABLE_FTS5")
    );
}
