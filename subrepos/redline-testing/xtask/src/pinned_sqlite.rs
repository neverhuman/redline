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

/// A verified pinned reference shell.
#[derive(Debug)]
pub struct PinnedSqlite {
    pub path: PathBuf,
    pub version: String,
    pub stamp: String,
}

/// The shell to bless against: `explicit` when given, else the pinned
/// reference build. Either way it must be the stamped pinned release.
pub fn resolve(repo_root: &Path, explicit: Option<&Path>) -> Result<PinnedSqlite> {
    let path = match explicit {
        Some(path) => path.to_path_buf(),
        None => default_path(repo_root, std::env::var_os(PREFIX_ENV).map(PathBuf::from))?,
    };
    verify(&path)
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
        .find(|dir| dir.join("scripts/sqlite/build-reference.sh").is_file())
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

/// Refuse any shell that is not the stamped pinned release.
fn verify(path: &Path) -> Result<PinnedSqlite> {
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
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{PINNED_SQLITE_VERSION, STAMP_FILE, default_path, resolve};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "xtask-pinned-sqlite-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A fake `sqlite3` under `<prefix>/bin` that prints `version`.
    fn fake_shell(prefix: &Path, version: &str, stamp: Option<&str>) -> PathBuf {
        let bin = prefix.join("bin/sqlite3");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(
            &bin,
            format!("#!/bin/sh\necho '{version} 2026-05-05 fake'\n"),
        )
        .unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        if let Some(stamp) = stamp {
            fs::write(prefix.join(STAMP_FILE), stamp).unwrap();
        }
        bin
    }

    #[test]
    fn default_is_the_reference_build_of_the_enclosing_repository() {
        let fixture = Fixture::new();
        let root = &fixture.0;
        fs::create_dir_all(root.join("scripts/sqlite")).unwrap();
        fs::write(root.join("scripts/sqlite/build-reference.sh"), "").unwrap();
        let subrepo = root.join("subrepos/redline-testing");
        fs::create_dir_all(&subrepo).unwrap();

        let path = default_path(&subrepo, None).unwrap();
        assert_eq!(
            path,
            root.join(format!(
                "target/sqlite-reference/{PINNED_SQLITE_VERSION}/bin/sqlite3"
            ))
        );
        assert_eq!(
            default_path(&subrepo, Some(PathBuf::from("/opt/ref"))).unwrap(),
            PathBuf::from("/opt/ref/bin/sqlite3")
        );
        // Outside a RedlineDB checkout there is no default: never PATH.
        let lone = Fixture::new();
        assert!(default_path(&lone.0, None).is_err());
    }

    #[test]
    fn stamped_pinned_shell_is_accepted() {
        let fixture = Fixture::new();
        let bin = fake_shell(&fixture.0, PINNED_SQLITE_VERSION, Some("abc\n-O2\n"));
        let pinned = resolve(&fixture.0, Some(&bin)).unwrap();
        assert_eq!(pinned.stamp, "abc\n-O2");
        assert!(pinned.version.starts_with(PINNED_SQLITE_VERSION));
    }

    #[test]
    fn unstamped_or_other_release_shells_are_refused() {
        let fixture = Fixture::new();
        let unstamped = fake_shell(&fixture.0.join("plain"), PINNED_SQLITE_VERSION, None);
        let error = resolve(&fixture.0, Some(&unstamped)).unwrap_err();
        assert!(
            error.to_string().contains("refusing unstamped"),
            "{error:#}"
        );

        let system = fake_shell(&fixture.0.join("system"), "3.45.1", Some("abc\n"));
        let error = resolve(&fixture.0, Some(&system)).unwrap_err();
        assert!(error.to_string().contains("3.45.1"), "{error:#}");

        let missing = fixture.0.join("nowhere/bin/sqlite3");
        assert!(resolve(&fixture.0, Some(&missing)).is_err());
    }
}
