//! What a run measured, beyond its binaries: the source tree the target and
//! runner were built from, the corpus, the verdict rules and the reference
//! build. Recorded once per run in both the run provenance and
//! official-evidence.json; `report` copies it and never re-probes it (SQ-03).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Schema of the per-suite run provenance (`provenance.json` next to the
/// raw results). v2 adds the source, corpus, policy and oracle-build
/// identity and the run's elapsed time.
pub(crate) const RUN_PROVENANCE_SCHEMA: &str = "redline-testing-run-provenance-v2";

/// The source inputs of the measured binaries, relative to the repository
/// root. This is the pathspec `ops/ci/sqlite-parity-report.sh` hashes into
/// `.github/parity-report-inputs.sha256`; a unit test below keeps the two
/// lists equal. Generated outputs (README, reports, charts,
/// benchmark results, `target/`) are outside it.
pub(crate) const SOURCE_INPUT_PATHS: [&str; 12] = [
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "crates",
    "subrepos",
    "metadata",
    "ops",
    "scripts",
    "agent/audit-policy.toml",
    ".jankurai/audit-policy.toml",
    ".github/workflows/ci.yml",
    ".github/workflows/sqlite-parity-report.yml",
];

/// At most this many `git status --porcelain` lines are recorded.
const MAX_DIRTY_PATHS: usize = 64;

/// The measured source tree. Every field is `None` when the run did not
/// start inside a Git checkout; a report treats that as unknown, not clean.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SourceIdentity {
    /// `git rev-parse HEAD`.
    pub(crate) source_commit: Option<String>,
    /// `git rev-parse HEAD^{tree}`.
    pub(crate) source_tree: Option<String>,
    /// SHA-256 of `git ls-tree -r HEAD -- <SOURCE_INPUT_PATHS>`, the same
    /// bytes `ops/ci/sqlite-parity-report.sh` pipes to `sha256sum`.
    pub(crate) source_inputs_sha256: Option<String>,
    /// Whether `git status --porcelain --untracked-files=all` reports any
    /// change under `SOURCE_INPUT_PATHS`, untracked files included.
    pub(crate) source_dirty: Option<bool>,
    /// The first `MAX_DIRTY_PATHS` porcelain lines, so dirtiness can be
    /// diagnosed from the evidence alone.
    pub(crate) source_dirty_paths: Vec<String>,
}

/// The run identity shared by the run provenance and official evidence.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct RunIdentity {
    #[serde(flatten)]
    pub(crate) source: SourceIdentity,
    pub(crate) corpus_sha256: String,
    pub(crate) assertion_policy_sha256: String,
    /// `<prefix>/.sqlite-reference-sha3` beside `<prefix>/bin/sqlite3`: the
    /// source archive SHA3-256 and compile flags of
    /// scripts/sqlite/build-reference.sh. `None` for any other sqlite3.
    pub(crate) oracle_build_stamp: Option<String>,
}

pub(crate) fn capture(sqlite_bin: &Path) -> RunIdentity {
    let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    RunIdentity {
        source: source_identity(&current_dir),
        corpus_sha256: crate::sqlite_parity::corpus_sha256(),
        assertion_policy_sha256: crate::sqlite_parity::assertion_policy_sha256(),
        oracle_build_stamp: oracle_build_stamp(sqlite_bin),
    }
}

/// The source identity of the Git checkout that contains `dir`.
pub(crate) fn source_identity(dir: &Path) -> SourceIdentity {
    let unknown = SourceIdentity {
        source_commit: None,
        source_tree: None,
        source_inputs_sha256: None,
        source_dirty: None,
        source_dirty_paths: Vec::new(),
    };
    let Some(toplevel) = git_text(dir, &["rev-parse", "--show-toplevel"]) else {
        return unknown;
    };
    let root = PathBuf::from(toplevel);
    let mut ls_tree = vec!["ls-tree", "-r", "HEAD", "--"];
    ls_tree.extend(SOURCE_INPUT_PATHS);
    let mut status = vec!["status", "--porcelain", "--untracked-files=all", "--"];
    status.extend(SOURCE_INPUT_PATHS);
    let status = git_bytes(&root, &status).map(|stdout| {
        String::from_utf8_lossy(&stdout)
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    });
    SourceIdentity {
        source_commit: git_text(&root, &["rev-parse", "HEAD"]),
        source_tree: git_text(&root, &["rev-parse", "HEAD^{tree}"]),
        source_inputs_sha256: git_bytes(&root, &ls_tree)
            .map(|stdout| format!("{:x}", Sha256::digest(&stdout))),
        source_dirty: status.as_ref().map(|lines| !lines.is_empty()),
        source_dirty_paths: status
            .unwrap_or_default()
            .into_iter()
            .take(MAX_DIRTY_PATHS)
            .collect(),
    }
}

/// The build stamp scripts/sqlite/build-reference.sh writes beside the
/// shell it built, trimmed of trailing whitespace.
pub(crate) fn oracle_build_stamp(sqlite_bin: &Path) -> Option<String> {
    let resolved = super::resolve_executable_path(sqlite_bin).ok()?;
    let prefix = resolved.parent()?.parent()?;
    let stamp = fs::read_to_string(prefix.join(".sqlite-reference-sha3")).ok()?;
    let stamp = stamp.trim_end();
    (!stamp.is_empty()).then(|| stamp.to_owned())
}

fn git_bytes(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn git_text(dir: &Path, args: &[&str]) -> Option<String> {
    git_bytes(dir, args)
        .map(|stdout| String::from_utf8_lossy(&stdout).trim().to_owned())
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::{SOURCE_INPUT_PATHS, oracle_build_stamp, source_identity};

    fn repository_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn source_inputs_match_the_report_publisher_recipe() {
        let script = fs::read_to_string(repository_root().join("ops/ci/sqlite-parity-report.sh"))
            .expect("read ops/ci/sqlite-parity-report.sh");
        let marker = "git ls-tree -r HEAD --";
        let start = script.find(marker).expect("inputs recipe") + marker.len();
        let end = script[start..]
            .find("| sha256sum")
            .expect("inputs recipe end");
        let paths = script[start..start + end]
            .split_whitespace()
            .filter(|token| *token != "\\")
            .collect::<Vec<_>>();
        assert_eq!(paths, SOURCE_INPUT_PATHS);
    }

    #[test]
    fn source_inputs_hash_is_the_shell_recipe() {
        let root = repository_root();
        let identity = source_identity(&root);
        let shell = Command::new("bash")
            .arg("-c")
            .arg(format!(
                "git ls-tree -r HEAD -- {} | sha256sum | cut -d ' ' -f 1",
                SOURCE_INPUT_PATHS.join(" ")
            ))
            .current_dir(&root)
            .output()
            .expect("run the shell recipe");
        assert!(shell.status.success());
        let expected = String::from_utf8_lossy(&shell.stdout).trim().to_owned();
        assert_eq!(
            identity.source_inputs_sha256.as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(identity.source_commit.as_ref().map(String::len), Some(40));
        assert_eq!(identity.source_tree.as_ref().map(String::len), Some(40));
        assert!(identity.source_dirty.is_some());
    }

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
}
