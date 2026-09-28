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
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{PINNED_SQLITE_VERSION, STAMP_FILE, default_path, resolve};
    use crate::test_support::{ScratchDir, write_executable};

    struct Fixture(ScratchDir);

    impl Fixture {
        fn new() -> Self {
            Self(ScratchDir::new("xtask-pinned-sqlite"))
        }

        fn root(&self) -> &Path {
            self.0.path()
        }
    }

    /// A fake `sqlite3` under `<prefix>/bin` that prints `version`.
    fn fake_shell(prefix: &Path, version: &str, stamp: Option<&str>) -> PathBuf {
        let bin = prefix.join("bin/sqlite3");
        write_executable(
            &bin,
            &format!("#!/bin/sh\necho '{version} 2026-05-05 fake'\n"),
        );
        if let Some(stamp) = stamp {
            fs::write(prefix.join(STAMP_FILE), stamp).unwrap();
        }
        bin
    }

    #[test]
    fn default_is_the_reference_build_of_the_enclosing_repository() {
        let fixture = Fixture::new();
        let root = fixture.root();
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
        // (The scratch fixtures live under this checkout's target
        // directory, so this one is a plain temporary directory; it holds
        // no executable.)
        let lone = std::env::temp_dir().join(format!("xtask-pinned-lone-{}", std::process::id()));
        fs::create_dir_all(&lone).unwrap();
        let outside = default_path(&lone, None);
        let _ = fs::remove_dir_all(&lone);
        assert!(outside.is_err());
    }

    /// A build-reference.sh in `root` whose stamp is `abc` then `flags`.
    fn build_script(root: &Path, flags: &str) {
        let script = root.join("scripts/sqlite/build-reference.sh");
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(
            script,
            format!(
                "version=\"3.53.1\"\narchive_sha3=\"abc\"\nsqlite_cflags=(\n  {}\n)\n",
                flags.split(' ').collect::<Vec<_>>().join("\n  ")
            ),
        )
        .unwrap();
    }

    #[test]
    fn stamped_pinned_shell_is_accepted() {
        let fixture = Fixture::new();
        build_script(fixture.root(), "-O2 -DSQLITE_ENABLE_FTS5");
        let bin = fake_shell(
            &fixture.root().join("ref"),
            PINNED_SQLITE_VERSION,
            Some("abc\n-O2 -DSQLITE_ENABLE_FTS5\n"),
        );
        let pinned = resolve(fixture.root(), Some(&bin)).unwrap();
        assert_eq!(pinned.stamp, "abc\n-O2 -DSQLITE_ENABLE_FTS5");
        assert!(pinned.version.starts_with(PINNED_SQLITE_VERSION));
    }

    #[test]
    fn a_stamp_other_than_the_build_scripts_is_refused() {
        // A 3.53.1 shell stamped by an older revision of the build script
        // (one flag fewer), or by hand: blessing against it would capture
        // what the official lane, which rebuilds on a stamp mismatch, never
        // scores against.
        let fixture = Fixture::new();
        build_script(
            fixture.root(),
            "-O2 -DSQLITE_ENABLE_FTS5 -DSQLITE_ENABLE_VFSTRACE",
        );
        for stamp in [
            "abc\n-O2 -DSQLITE_ENABLE_FTS5\n",
            "def\n-O2 -DSQLITE_ENABLE_FTS5 -DSQLITE_ENABLE_VFSTRACE\n",
        ] {
            let bin = fake_shell(
                &fixture.root().join("stale"),
                PINNED_SQLITE_VERSION,
                Some(stamp),
            );
            let error = resolve(fixture.root(), Some(&bin))
                .expect_err("a stamp the build script would not write");
            assert!(
                format!("{error:#}")
                    .contains("does not match the stamp scripts/sqlite/build-reference.sh writes"),
                "{stamp:?}: {error:#}"
            );
        }
    }

    #[test]
    fn the_expected_stamp_is_what_the_build_script_prints() {
        // Parse this checkout's build-reference.sh, and let bash print its
        // reference_stamp_text from the same definitions: the two agree.
        let script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .map(|dir| dir.join("scripts/sqlite/build-reference.sh"))
            .find(|path| path.is_file())
            .expect("scripts/sqlite/build-reference.sh above xtask");
        let parsed = super::expected_stamp(&script).expect("parse build script");
        let output = std::process::Command::new("bash")
            .arg("-c")
            .arg(r#"eval "$(sed -n '/^archive_sha3=/p; /^sqlite_cflags=(/,/^)/p' "$1")"; printf '%s\n%s\n' "$archive_sha3" "${sqlite_cflags[*]}""#)
            .arg("bash")
            .arg(&script)
            .output()
            .expect("run bash");
        assert!(output.status.success());
        let printed = String::from_utf8(output.stdout).unwrap();
        assert_eq!(parsed, printed.trim_end());
        assert!(parsed.contains("-DSQLITE_ENABLE_FTS5"), "{parsed}");
    }

    #[test]
    fn unstamped_or_other_release_shells_are_refused() {
        let fixture = Fixture::new();
        build_script(fixture.root(), "-O2");
        let unstamped = fake_shell(&fixture.root().join("plain"), PINNED_SQLITE_VERSION, None);
        let error = resolve(fixture.root(), Some(&unstamped)).unwrap_err();
        assert!(
            error.to_string().contains("refusing unstamped"),
            "{error:#}"
        );

        let system = fake_shell(&fixture.root().join("system"), "3.45.1", Some("abc\n-O2\n"));
        let error = resolve(fixture.root(), Some(&system)).unwrap_err();
        assert!(error.to_string().contains("3.45.1"), "{error:#}");

        let missing = fixture.root().join("nowhere/bin/sqlite3");
        assert!(resolve(fixture.root(), Some(&missing)).is_err());
    }
}
