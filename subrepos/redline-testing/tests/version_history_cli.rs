//! Black-box `redline-testing version-history` (L-05): the subcommand
//! exists, reads only a bundle's summary.json, and never touches a README
//! it cannot render into.

use std::fs;
use std::process::Command;

use tempfile::TempDir;

fn version_history(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_redline-testing"))
        .arg("version-history")
        .args(args)
        .output()
        .expect("run redline-testing version-history")
}

#[test]
fn a_directory_without_a_summary_is_refused_and_the_readme_is_untouched() {
    let dir = TempDir::new().expect("tempdir");
    let bundle = dir.path().join("bundle");
    fs::create_dir(&bundle).expect("mkdir");
    let readme = dir.path().join("README.md");
    let text = "<!-- version-history:begin -->\nkept\n<!-- version-history:end -->\n";
    fs::write(&readme, text).expect("write");
    let output = version_history(&[
        "--bundle",
        bundle.to_str().expect("utf-8"),
        "--readme",
        readme.to_str().expect("utf-8"),
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("summary.json"), "{stderr}");
    assert_eq!(fs::read_to_string(&readme).expect("read"), text);
}

#[test]
fn check_without_a_readme_is_refused() {
    let dir = TempDir::new().expect("tempdir");
    let output = version_history(&["--bundle", dir.path().to_str().expect("utf-8"), "--check"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--readme"), "{stderr}");
}
