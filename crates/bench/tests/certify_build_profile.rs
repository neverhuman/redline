//! Certification numbers come only from an optimized benchmark (BM3-05).
//!
//! certify's measured children exec the parent binary (scheduler dispatch
//! runs `current_exe`), so a debug parent times debug children. A debug
//! build refuses to certify unless it is told the run is a diagnostic, and
//! every recipe that runs certify builds it with `--release`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

/// One engine, one workload, one thread, one second: the smallest certify.
fn tiny_config(dir: &Path) -> PathBuf {
    let path = dir.join("tiny.toml");
    fs::write(
        &path,
        format!(
            "out_dir = \"{}\"\nengines = [\"redline\"]\nworkloads = [\"point-read-pk\"]\ndurabilities = [\"normal\"]\nthreads = [1]\nrows = 16\nseconds = 1\ncache_mib = 1\n",
            dir.join("compare").display()
        ),
    )
    .expect("write tiny config");
    path
}

fn certify(config: &Path, out_dir: &Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_redlinedb-bench"))
        .args(["certify", "--config"])
        .arg(config)
        .arg("--out-dir")
        .arg(out_dir)
        .args(["--seed", "7", "--repetitions", "1", "--warmup", "0"])
        .args(extra)
        .output()
        .expect("run redlinedb-bench certify")
}

#[test]
fn certify_refuses_a_debug_build_unless_marked_diagnostic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = tiny_config(dir.path());
    let out_dir = dir.path().join("certify");
    let output = certify(&config, &out_dir, &[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if cfg!(debug_assertions) {
        assert!(
            !output.status.success(),
            "a debug certify ran to completion:\n{stderr}"
        );
        assert!(stderr.contains("debug build"), "{stderr}");
        assert!(
            !out_dir.join("manifest.json").exists(),
            "a refused certify still wrote a manifest"
        );

        let diagnostic = dir.path().join("certify-diagnostic");
        let output = certify(&config, &diagnostic, &["--allow-debug-build"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let manifest: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(diagnostic.join("manifest.json")).expect("manifest"),
        )
        .expect("manifest JSON");
        assert_eq!(manifest["build_profile"], "debug");
        assert_eq!(manifest["publishable"], false);
    } else {
        assert!(output.status.success(), "{stderr}");
        let manifest: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(out_dir.join("manifest.json")).expect("manifest"),
        )
        .expect("manifest JSON");
        assert_eq!(manifest["build_profile"], "release");
        assert_eq!(manifest["publishable"], true);
    }
}

#[test]
fn every_certify_recipe_builds_release() {
    let root = repository_root();
    for file in [
        "scripts/just/run.sh",
        ".jankurai/proof-lanes.toml",
        ".jankurai/test-map.json",
        "docs/testing.md",
    ] {
        let text = fs::read_to_string(root.join(file)).expect("read recipe file");
        let certify_lines = text
            .lines()
            // `cargo run -p redlinedb-bench [flags] -- certify ...`; a test
            // target named certify_* is not a certify run.
            .filter(|line| line.contains("redlinedb-bench") && line.contains("-- certify "))
            .collect::<Vec<_>>();
        assert!(!certify_lines.is_empty(), "{file} runs no certify");
        for line in certify_lines {
            let before_args = line.split(" -- ").next().unwrap_or(line);
            assert!(
                before_args.contains("--release"),
                "{file} runs certify without --release: {}",
                line.trim()
            );
        }
    }
}
