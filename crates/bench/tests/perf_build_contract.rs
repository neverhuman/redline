//! Perf builds declare the flags they were built with (BM3-05).
//!
//! Cargo takes CARGO_ENCODED_RUSTFLAGS over RUSTFLAGS over the target's
//! config rustflags, so a build that inherits the caller's environment can
//! be tuned without saying so. Every W2 leg that the matrix builds itself
//! sets RUSTFLAGS explicitly, with CARGO_ENCODED_RUSTFLAGS cleared, and
//! records exactly those flags.

use std::path::PathBuf;
use std::process::Command;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

#[test]
fn w2_legs_build_with_explicit_rustflags() {
    let root = repository_root();
    let output = Command::new("bash")
        .args([
            "scripts/perf/w2-matrix.sh",
            "--dry-run",
            "--suite",
            "none",
            "--profiles",
            "release,release-native",
            "--allocators",
            "mimalloc",
        ])
        .current_dir(&root)
        .env("RUSTFLAGS", "-C target-cpu=native")
        .env("CARGO_ENCODED_RUSTFLAGS", "-Ctarget-cpu=native")
        .output()
        .expect("run w2-matrix.sh --dry-run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let builds = stdout
        .lines()
        .filter(|line| line.contains("cargo build"))
        .collect::<Vec<_>>();
    assert_eq!(builds.len(), 2, "{stdout}");
    // The portable leg clears every inherited flag.
    assert!(
        builds[0].starts_with("+ env -u CARGO_ENCODED_RUSTFLAGS RUSTFLAGS= cargo build --release"),
        "{}",
        builds[0]
    );
    // The native leg sets exactly the shared base flags.
    assert!(
        builds[1].starts_with("+ env -u CARGO_ENCODED_RUSTFLAGS RUSTFLAGS=-C\\ link-arg=-fuse-ld=mold\\ -C\\ target-cpu=native cargo build --profile release-native"),
        "{}",
        builds[1]
    );
}
