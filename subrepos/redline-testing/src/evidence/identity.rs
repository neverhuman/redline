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
/// root. This is the pathspec `ops/ci/source-inputs-sha256.sh` hashes (into
/// `.github/parity-report-inputs.sha256`); a unit test below keeps the two
/// lists equal. Generated outputs (README, reports, charts,
/// benchmark results, `target/`) are outside it.
pub(crate) const SOURCE_INPUT_PATHS: [&str; 13] = [
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    // Cargo configuration (linker, target-cpu, rustflags) both measured
    // builds read from the repository root.
    ".cargo",
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
    /// bytes `ops/ci/source-inputs-sha256.sh` pipes to `sha256sum`.
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
    /// The directory the identity was taken in, where `still_unchanged`
    /// takes it again. Not recorded.
    #[serde(skip)]
    pub(crate) captured_in: PathBuf,
}

/// The run identity of the checkout that contains the working directory.
/// `run` captures it once, before any suite runs, and every evidence file
/// of the run records that one value.
pub(crate) fn capture(sqlite_bin: &Path) -> RunIdentity {
    let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    capture_in(&current_dir, sqlite_bin)
}

pub(crate) fn capture_in(dir: &Path, sqlite_bin: &Path) -> RunIdentity {
    RunIdentity {
        source: source_identity(dir),
        corpus_sha256: crate::sqlite_parity::corpus_sha256(),
        assertion_policy_sha256: crate::sqlite_parity::assertion_policy_sha256(),
        oracle_build_stamp: oracle_build_stamp(sqlite_bin),
        captured_in: dir.to_path_buf(),
    }
}

/// `ensure_unchanged` against the identity taken again where `start` was.
pub(crate) fn still_unchanged(start: &RunIdentity, sqlite_bin: &Path) -> anyhow::Result<()> {
    ensure_unchanged(start, &capture_in(&start.captured_in, sqlite_bin))
}

/// Fails when the committed source or the reference build a run started
/// from is no longer what `start` recorded: a commit or checkout made in
/// the same working tree while the run was going would otherwise be
/// credited with binaries built before it. Uncommitted edits are already
/// in `start.source.source_dirty`.
pub(crate) fn ensure_unchanged(start: &RunIdentity, now: &RunIdentity) -> anyhow::Result<()> {
    let fields = [
        (
            "source commit",
            &start.source.source_commit,
            &now.source.source_commit,
        ),
        (
            "source tree",
            &start.source.source_tree,
            &now.source.source_tree,
        ),
        (
            "source inputs hash",
            &start.source.source_inputs_sha256,
            &now.source.source_inputs_sha256,
        ),
        (
            "oracle build stamp",
            &start.oracle_build_stamp,
            &now.oracle_build_stamp,
        ),
    ];
    for (field, before, after) in fields {
        if before != after {
            anyhow::bail!(
                "the {field} changed during the run ({before:?} when it started, {after:?} now): \
                 the measured binaries cannot be attributed to one source, so no evidence is \
                 written; rerun from a checkout nothing commits to while the run is going"
            );
        }
    }
    Ok(())
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

    use super::{SOURCE_INPUT_PATHS, source_identity};

    fn repository_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn source_inputs_match_the_report_publisher_recipe() {
        let script = fs::read_to_string(repository_root().join("ops/ci/source-inputs-sha256.sh"))
            .expect("read ops/ci/source-inputs-sha256.sh");
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
}

#[cfg(test)]
#[path = "identity_repo_tests.rs"]
mod repo_tests;
