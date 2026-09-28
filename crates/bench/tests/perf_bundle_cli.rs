//! The perf_evidence subcommands behind scripts/perf/release-bench.sh
//! (L-05): `case-list-ids` prints a case list as the five-digit ids the
//! runner takes, and `summarize-bundle` refuses a directory that is not a
//! bundle rather than writing a summary.

use std::fs;
use std::process::Command;

use tempfile::TempDir;

fn perf_evidence(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_perf_evidence"))
        .args(args)
        .output()
        .expect("run perf_evidence")
}

#[test]
fn case_list_ids_prints_five_digit_ids_and_refuses_a_bad_list() {
    let dir = TempDir::new().expect("tempdir");
    let list = dir.path().join("cases.txt");
    fs::write(&list, "# cohort\n00150  # P1-worst\n7\n\n").expect("write");
    let output = perf_evidence(&["case-list-ids", list.to_str().expect("utf-8 path")]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "00007\n00150\n");

    fs::write(&list, "00150\n00150\n").expect("write");
    let output = perf_evidence(&["case-list-ids", list.to_str().expect("utf-8 path")]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("listed twice"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn summarize_bundle_refuses_a_directory_without_a_bundle() {
    let dir = TempDir::new().expect("tempdir");
    let output = perf_evidence(&[
        "summarize-bundle",
        "--bundle",
        dir.path().to_str().expect("utf-8 path"),
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("bundle.json"), "{stderr}");
    assert!(!dir.path().join("summary.json").exists());
}
