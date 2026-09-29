//! The SQLite shell the corpus is blessed against.
//!
//! `generate` captures expected outputs from a shell and `ship-gate` checks
//! them against one, so both must use the same oracle as the official
//! runner: the sqlite3 that `scripts/sqlite/build-reference.sh` builds and
//! stamps. A bare `sqlite3` from PATH is some other build (Ubuntu ships
//! 3.45.1 with different compile options), and blessing against it is how
//! the corpus drifted from the oracle it is scored against.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// The SQLite release `scripts/sqlite/build-reference.sh` builds.
pub const PINNED_SQLITE_VERSION: &str = "3.53.1";

/// Build stamp the reference build writes beside `bin/`: the source archive
/// SHA3-256 and the compile flags.
const STAMP_FILE: &str = ".sqlite-reference-sha3";

/// Environment variable `build-reference.sh` reads for its install prefix.
const PREFIX_ENV: &str = "REDLINEDB_SQLITE_REFERENCE_PREFIX";

/// The script that builds and stamps the reference shell, relative to the
/// RedlineDB checkout root.
const BUILD_SCRIPT: &str = "scripts/sqlite/build-reference.sh";

/// A verified pinned reference shell.
#[derive(Debug)]
pub struct PinnedSqlite {
    pub path: PathBuf,
    pub version: String,
    pub stamp: String,
}

/// The shell to bless against: `explicit` when given, else the pinned
/// reference build. Either way it must be the pinned release, stamped
/// exactly as the enclosing checkout's build-reference.sh stamps it now.
pub fn resolve(repo_root: &Path, explicit: Option<&Path>) -> Result<PinnedSqlite> {
    let path = match explicit {
        Some(path) => path.to_path_buf(),
        None => default_path(repo_root, std::env::var_os(PREFIX_ENV).map(PathBuf::from))?,
    };
    let script = build_script(repo_root)?;
    verify(&path, &expected_stamp(&script)?)
}

/// `scripts/sqlite/build-reference.sh` in the nearest ancestor of
/// `repo_root` that has one.
fn build_script(repo_root: &Path) -> Result<PathBuf> {
    repo_root
        .ancestors()
        .map(|dir| dir.join(BUILD_SCRIPT))
        .find(|script| script.is_file())
        .with_context(|| {
            format!(
                "no {BUILD_SCRIPT} above {}, so the pinned reference build's stamp is unknown",
                repo_root.display()
            )
        })
}

/// The stamp `build-reference.sh` writes beside the shell it builds, as its
/// `reference_stamp_text` prints it: the source archive SHA3-256, then the
/// `sqlite_cflags` joined by spaces (trailing newline trimmed). A shell whose
/// stamp differs was built from another archive or with other flags, and
/// the script itself would rebuild it before the official lane scores.
fn expected_stamp(script: &Path) -> Result<String> {
    let text = fs::read_to_string(script).with_context(|| format!("read {}", script.display()))?;
    let archive_sha3 = text
        .lines()
        .find_map(|line| line.strip_prefix("archive_sha3="))
        .map(|value| value.trim().trim_matches('"'))
        .filter(|value| !value.is_empty())
        .with_context(|| format!("{} defines no archive_sha3", script.display()))?;
    let flags = text
        .split_once("\nsqlite_cflags=(")
        .and_then(|(_, rest)| rest.split_once("\n)"))
        .map(|(body, _)| body.split_whitespace().collect::<Vec<_>>())
        .filter(|flags| !flags.is_empty())
        .with_context(|| format!("{} defines no sqlite_cflags", script.display()))?;
    Ok(format!("{archive_sha3}\n{}", flags.join(" ")))
}

/// `$REDLINEDB_SQLITE_REFERENCE_PREFIX/bin/sqlite3`, else
/// `<root>/target/sqlite-reference/<version>/bin/sqlite3` under the nearest
/// ancestor of `repo_root` that holds `scripts/sqlite/build-reference.sh`.
fn default_path(repo_root: &Path, prefix: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(prefix) = prefix {
        return Ok(prefix.join("bin").join("sqlite3"));
    }
    let Some(root) = repo_root
        .ancestors()
        .find(|dir| dir.join(BUILD_SCRIPT).is_file())
    else {
        bail!(
            "no scripts/sqlite/build-reference.sh above {}; pass --sqlite-bin with the stamped \
             sqlite3 {PINNED_SQLITE_VERSION} reference shell",
            repo_root.display()
        );
    };
    Ok(root
        .join("target/sqlite-reference")
        .join(PINNED_SQLITE_VERSION)
        .join("bin/sqlite3"))
}

/// Refuse any shell that is not the pinned release stamped `expected`.
fn verify(path: &Path, expected: &str) -> Result<PinnedSqlite> {
    let resolved = fs::canonicalize(path).with_context(|| {
        format!(
            "sqlite3 reference shell {} is missing; build it with \
             `bash scripts/sqlite/build-reference.sh`",
            path.display()
        )
    })?;
    let stamp_path = resolved
        .parent()
        .and_then(Path::parent)
        .map(|prefix| prefix.join(STAMP_FILE));
    let stamp = stamp_path
        .as_deref()
        .and_then(|stamp| fs::read_to_string(stamp).ok())
        .map(|stamp| stamp.trim_end().to_owned())
        .filter(|stamp| !stamp.is_empty());
    let Some(stamp) = stamp else {
        bail!(
            "refusing unstamped sqlite3 shell {}: no {STAMP_FILE} beside its bin/ directory. \
             The corpus is blessed only against the sqlite3 {PINNED_SQLITE_VERSION} reference \
             that `bash scripts/sqlite/build-reference.sh` builds and stamps",
            resolved.display()
        );
    };
    if stamp != expected {
        bail!(
            "refusing sqlite3 shell {}: its {STAMP_FILE} {stamp:?} does not match the stamp \
             {BUILD_SCRIPT} writes now ({expected:?}), so it was built from another archive or \
             with other compile flags. Rebuild it with `bash {BUILD_SCRIPT}`",
            resolved.display()
        );
    }
    let output = Command::new(&resolved)
        .arg("--version")
        .output()
        .with_context(|| format!("run {} --version", resolved.display()))?;
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || version.split_whitespace().next() != Some(PINNED_SQLITE_VERSION)
    {
        bail!(
            "refusing sqlite3 shell {}: it reports version {version:?}, but the corpus is \
             blessed against sqlite3 {PINNED_SQLITE_VERSION}",
            resolved.display()
        );
    }
    Ok(PinnedSqlite {
        path: resolved,
        version,
        stamp,
    })
}

#[cfg(test)]
#[path = "pinned_sqlite_tests.rs"]
mod tests;
