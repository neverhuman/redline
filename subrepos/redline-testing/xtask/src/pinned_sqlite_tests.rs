//! Tests for resolving the pinned SQLite shell: the default path, the
//! build stamp it must carry, and the shells that are refused.

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
