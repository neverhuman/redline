//! A release bench bundle's summary (L-05) is re-derived from its files,
//! and a bundle that is not what it claims is refused.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::*;
use crate::perf_evidence::bundle_stats::NoiseVerdict;

const REFERENCE: &str = "reference-sha";
const RUNNER: &str = "runner-sha";
const REFERENCE_NS: u64 = 1_000_000;
/// Base ratios of cases 10001..=10004 for a label with factor 1.
const BASE: [f64; 4] = [2.0, 3.0, 4.0, 5.0];
const CASES: [&str; 4] = ["10001", "10002", "10003", "10004"];

struct LabelSpec {
    name: &'static str,
    factor: f64,
    /// (run, case) samples that fail.
    failures: Vec<(usize, &'static str)>,
}

fn label(name: &'static str, factor: f64) -> LabelSpec {
    LabelSpec {
        name,
        factor,
        failures: Vec::new(),
    }
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(value).unwrap()),
    )
    .expect("write");
}

fn read_value(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).expect("read")).expect("json")
}

fn edit_json(path: &Path, edit: impl FnOnce(&mut Value)) {
    let mut value = read_value(path);
    edit(&mut value);
    write_json(path, &value);
}

fn binary_sha(label: &str) -> String {
    format!("{label}-binary-sha")
}

/// The ratio a label's case shows in a run: its base ratio scaled by the
/// label's factor, plus 0.1 per run after the first.
fn ratio(spec: &LabelSpec, run: usize, case_index: usize) -> f64 {
    BASE[case_index] * spec.factor + 0.1 * (run - 1) as f64
}

fn row(
    case: &str,
    role: &str,
    index: usize,
    repetition: Option<usize>,
    target_ns: u64,
    status: &str,
    target_sha: &str,
) -> String {
    json!({
        "case_id": case,
        "status": status,
        "sample_role": role,
        "sample_index": index,
        "repetition_index": repetition,
        "reference_elapsed_ns": REFERENCE_NS,
        "target_elapsed_ns": target_ns,
        "latency_ratio": target_ns as f64 / REFERENCE_NS as f64,
        "target_executable_sha256": target_sha,
        "reference_executable_sha256": REFERENCE,
    })
    .to_string()
}

/// Writes a raw file with one warmup and three measured repetitions per
/// case, and the runner's completion marker.
fn write_raw(path: &Path, spec: &LabelSpec, run: usize, cases: &[&str]) {
    let target_sha = binary_sha(spec.name);
    let mut text = String::new();
    for (index, case) in cases.iter().enumerate() {
        let target_ns = (ratio(spec, run, index) * REFERENCE_NS as f64).round() as u64;
        let status = if spec.failures.contains(&(run, *case)) {
            "failed"
        } else {
            "passed"
        };
        text.push_str(&row(
            case,
            "warmup",
            0,
            None,
            target_ns,
            status,
            &target_sha,
        ));
        text.push('\n');
        for repetition in 1..=3 {
            let role = format!("measured:{repetition}");
            text.push_str(&row(
                case,
                &role,
                repetition,
                Some(repetition),
                target_ns,
                status,
                &target_sha,
            ));
            text.push('\n');
        }
    }
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, &text).expect("write raw");
    write_marker(path, cases.len());
}

fn write_marker(raw: &Path, cases: usize) {
    let text = fs::read(raw).expect("read raw");
    let records = text.iter().filter(|byte| **byte == b'\n').count();
    write_json(
        &raw.with_file_name("raw.jsonl.complete.json"),
        &json!({
            "schema_version": "redline-testing-raw-complete-v1",
            "suite": "sqlite_parity",
            "raw_file": "raw.jsonl",
            "records": records,
            "cases": cases,
            "raw_sha256": sha(&text),
        }),
    );
}

struct Bundle {
    dir: TempDir,
}

impl Bundle {
    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn summarize(&self) -> Result<BundleSummary> {
        summarize_bundle(self.path())
    }

    fn error(&self) -> String {
        format!(
            "{:#}",
            self.summarize().expect_err("the bundle must be refused")
        )
    }
}

/// A complete bundle: `labels` in order, `runs` interleaved runs each, over
/// cases 10001..=10004 with a medium cohort of 10001 and 10002.
fn bundle(labels: &[LabelSpec], runs: usize) -> Bundle {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write_json(
        &root.join("cases.json"),
        &Value::Array(
            CASES
                .iter()
                .map(|case| json!({"id": case.parse::<u64>().unwrap()}))
                .collect(),
        ),
    );
    let medium = "# fixture cohort\n10001  # P0\n10002\n";
    fs::create_dir_all(root.join("cohorts")).expect("mkdir");
    fs::write(root.join("cohorts/medium-set.txt"), medium).expect("write cohort");

    let mut label_entries = Vec::new();
    let mut run_entries = Vec::new();
    let mut host_runs = Vec::new();
    for spec in labels {
        let binary = binary_sha(spec.name);
        write_json(
            &root.join(spec.name).join("build-contract.json"),
            &json!({
                "schema_version": "redline-perf-build-contract-v1",
                "captured_at_utc": "2026-09-28T00:00:00Z",
                "toolchain": {"rustc_verbose_version": ["rustc 1.95.0 (host)"]},
                "target": {
                    "path": format!("/bin/{}", spec.name), "sha256": binary, "size_bytes": 1,
                    "version": format!("redlinedb {}", spec.name),
                    "build": {"declared": true, "profile": "release", "features": null, "rustflags": ""}
                },
                "reference": {"path": "/bin/sqlite3", "sha256": REFERENCE, "size_bytes": 1, "version": "3.53.1", "compile_options": []},
                "runner": {"path": "/bin/redline-testing", "sha256": RUNNER, "size_bytes": 1, "version": "redline-testing 1.0.1"},
                "optimization": {"pgo_training_corpus": null}
            }),
        );
        write_json(
            &root.join(spec.name).join("build.json"),
            &json!({
                "schema_version": "redline-version-build-v1",
                "label": spec.name,
                "source_ref": spec.name,
                "source_commit": format!("{}-commit", spec.name),
                "binary_sha256": binary,
                "rustc_verbose_version": ["rustc 1.95.0 (build)", "host: x86_64-unknown-linux-gnu"],
                "profile": "release",
                "rustflags": "",
                "pgo": false
            }),
        );
        label_entries.push(json!({
            "label": spec.name,
            "binary": format!("/bin/{}", spec.name),
            "binary_sha256": binary,
            "source_ref": spec.name,
            "source_commit": format!("{}-commit", spec.name),
            "build_contract": format!("{}/build-contract.json", spec.name),
            "build_record": format!("{}/build.json", spec.name),
            "durability_env": true,
        }));
    }
    let mut sequence = 0;
    for run in 1..=runs {
        for spec in labels {
            sequence += 1;
            let raw = format!("{}/run-{run}/raw.jsonl", spec.name);
            write_raw(&root.join(&raw), spec, run, &CASES);
            let failed = spec
                .failures
                .iter()
                .any(|(failed_run, _)| *failed_run == run);
            run_entries
                .push(json!({"sequence": sequence, "label": spec.name, "run": run, "raw": raw}));
            host_runs.push(json!({
                "sequence": sequence, "label": spec.name, "run": run,
                "started_at_utc": "2026-09-28T01:00:00Z", "finished_at_utc": "2026-09-28T01:01:00Z",
                "loadavg_before": [1.0, 1.0, 1.0], "loadavg_after": [2.0, 1.0, 1.0],
                "runner_jobs_before": 0, "runner_jobs_after": 0,
                "runner_exit": i32::from(failed), "accepted": true, "reason": null
            }));
        }
    }
    write_json(
        &root.join("host.json"),
        &json!({
            "schema_version": "redline-release-bench-host-v1",
            "hostname": "fixture", "kernel": "Linux 6.8.0", "cpu_model": "Fixture CPU", "nproc": 8,
            "pinned_cpus": "2-5", "tmp_filesystem": "tmpfs", "governors": {"cpu2": "performance"},
            "runner_units_active": [], "max_loadavg": 32.0, "runs": host_runs
        }),
    );
    write_json(
        &root.join("bundle.json"),
        &json!({
            "schema_version": "redline-release-bench-bundle-v1",
            "bundle": "fixture",
            "started_at_utc": "2026-09-28T01:00:00Z",
            "finished_at_utc": "2026-09-28T02:00:00Z",
            "runs_per_label": runs,
            "protocol": {
                "suite": "sqlite_parity", "workers": 1, "repetitions": 3, "warmup": 1,
                "order": "alternate", "cpus": "2-5", "tmp_root": "tmpfs", "durability": "normal",
                "measurement_boundary": "cli_case_wall_time", "max_loadavg": 32.0,
                "command": "redline-testing run --suite sqlite_parity"
            },
            "cases": {"manifest": "cases.json", "count": CASES.len(), "case_list": null},
            "medium_cohort": {"path": "cohorts/medium-set.txt", "sha256": sha(medium.as_bytes()), "source": "fixture"},
            "labels": label_entries,
            "runs": run_entries
        }),
    );
    Bundle { dir }
}

/// `old`, then `new` at half its ratios; `new` fails case 10004 in run 2.
fn two_labels() -> Vec<LabelSpec> {
    let mut new = label("new", 0.5);
    new.failures.push((2, "10004"));
    vec![label("old", 1.0), new]
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

#[test]
fn summary_compares_labels_on_the_common_pass_set_across_runs() {
    let summary = bundle(&two_labels(), 3).summarize().expect("summary");
    assert_eq!(summary.corpus.cases, 4);
    // 10004 failed in one run of `new`, so only 10001..=10003 are common.
    assert_eq!(summary.common_pass_set.cases, 3);
    let (old, new) = (&summary.labels[0], &summary.labels[1]);
    assert_eq!((old.passed_cases, old.flaky_cases), (4, 0));
    assert_eq!((new.passed_cases, new.flaky_cases), (3, 1));
    assert_eq!(new.runs[1].failed, 1);
    assert_eq!(new.runs[1].runner_exit, 1);

    // old: case ratios 2, 3, 4 plus 0.1 per run; the median case is 3.
    assert_eq!(old.common.cases, 3);
    close(old.common.median.value, 3.1);
    close(old.common.median.min, 3.0);
    close(old.common.median.max, 3.2);
    assert_eq!(old.common.median.per_run.len(), 3);
    // Nearest-rank p95 of three cases is the largest.
    close(old.common.p95.value, 4.1);
    assert!(old.common.delta_vs_previous.is_none());

    close(new.common.median.value, 1.6);
    let delta = new.common.delta_vs_previous.as_ref().expect("delta");
    assert_eq!(delta.previous, "old");
    assert_eq!(delta.verdict, NoiseVerdict::ExceedsNoise);
    close(delta.median_change_pct, (1.6 / 3.1 - 1.0) * 100.0);

    // The medium cohort (10001, 10002) is sliced from the same runs.
    let medium = summary.medium_cohort.as_ref().expect("medium cohort");
    assert_eq!(
        (medium.listed, medium.in_bundle, medium.in_common_pass_set),
        (2, 2, 2)
    );
    close(old.medium.as_ref().expect("old medium").median.value, 2.6);
    close(new.medium.as_ref().expect("new medium").median.value, 1.35);

    assert_eq!(summary.reference.sha256, REFERENCE);
    assert_eq!(summary.runner.sha256, RUNNER);
    assert_eq!(new.build_rustc.as_deref(), Some("rustc 1.95.0 (build)"));
    assert!(summary.publishable, "{:?}", summary.publication_blockers);
}

#[test]
fn a_change_inside_the_run_spread_is_within_noise() {
    let labels = vec![label("a", 0.5), label("b", 0.51)];
    let summary = bundle(&labels, 3).summarize().expect("summary");
    let delta = summary.labels[1]
        .common
        .delta_vs_previous
        .as_ref()
        .expect("delta");
    assert_eq!(delta.verdict, NoiseVerdict::WithinNoise);
}

#[test]
fn one_run_measures_no_spread_and_is_not_publishable() {
    let summary = bundle(&two_labels(), 1).summarize().expect("summary");
    let delta = summary.labels[1]
        .common
        .delta_vs_previous
        .as_ref()
        .expect("delta");
    assert_eq!(delta.verdict, NoiseVerdict::Unassessed);
    assert!(!summary.publishable);
    assert!(
        summary
            .publication_blockers
            .iter()
            .any(|blocker| blocker.contains("1 run(s) per label")),
        "{:?}",
        summary.publication_blockers
    );
}

#[test]
fn check_mode_refuses_a_summary_the_files_do_not_support() {
    let bundle = bundle(&two_labels(), 3);
    write_bundle_summary(bundle.path(), false).expect("write");
    write_bundle_summary(bundle.path(), true).expect("an unchanged summary checks");
    edit_json(&bundle.path().join("summary.json"), |summary| {
        summary["common_pass_set"]["cases"] = json!(4);
    });
    let error = write_bundle_summary(bundle.path(), true).expect_err("drift");
    assert!(format!("{error:#}").contains("is not what the bundle's files support"));
}

#[test]
fn an_incomplete_or_unmarked_run_is_refused() {
    let incomplete = bundle(&two_labels(), 3);
    let raw = incomplete.path().join("old/run-2/raw.jsonl");
    let text = fs::read_to_string(&raw).expect("read");
    let kept = text
        .lines()
        .filter(|line| !line.contains("\"measured:3\""))
        .collect::<Vec<_>>();
    fs::write(&raw, kept.join("\n") + "\n").expect("write");
    write_marker(&raw, CASES.len());
    let error = incomplete.error();
    assert!(
        error.contains("expected measured repetitions 1..=3"),
        "{error}"
    );

    let unmarked = bundle(&two_labels(), 3);
    fs::remove_file(unmarked.path().join("new/run-3/raw.jsonl.complete.json")).expect("rm");
    let error = unmarked.error();
    assert!(error.contains("completion marker"), "{error}");
}

#[test]
fn a_run_of_another_binary_is_refused() {
    let bundle = bundle(&two_labels(), 3);
    // Label new's run 1 replaced by label old's records.
    let old = bundle.path().join("old/run-1/raw.jsonl");
    let new = bundle.path().join("new/run-1/raw.jsonl");
    fs::copy(&old, &new).expect("copy");
    write_marker(&new, CASES.len());
    let error = bundle.error();
    assert!(
        error.contains("target executables") && error.contains("new-binary-sha"),
        "{error}"
    );
}

#[test]
fn labels_must_share_one_reference_and_one_runner() {
    let bundle = bundle(&two_labels(), 3);
    edit_json(&bundle.path().join("new/build-contract.json"), |contract| {
        contract["reference"]["sha256"] = json!("another-reference");
    });
    let error = bundle.error();
    assert!(error.contains("not the bundle's reference-sha"), "{error}");
}

#[test]
fn a_rejected_or_overloaded_run_is_refused() {
    let rejected = bundle(&two_labels(), 3);
    edit_json(&rejected.path().join("host.json"), |host| {
        host["runs"][2]["accepted"] = json!(false);
        host["runs"][2]["reason"] = json!("load 40 above 32");
    });
    let error = rejected.error();
    assert!(error.contains("was rejected: load 40 above 32"), "{error}");

    let overloaded = bundle(&two_labels(), 3);
    edit_json(&overloaded.path().join("host.json"), |host| {
        host["runs"][0]["loadavg_after"] = json!([33.5, 1.0, 1.0]);
    });
    let error = overloaded.error();
    assert!(error.contains("exceeds the threshold 32"), "{error}");
}

#[test]
fn an_unexplained_runner_exit_is_refused() {
    let bundle = bundle(&two_labels(), 3);
    edit_json(&bundle.path().join("host.json"), |host| {
        host["runs"][0]["runner_exit"] = json!(1);
    });
    let error = bundle.error();
    assert!(
        error.contains("exited 1 although no case failed"),
        "{error}"
    );
}

#[test]
fn every_label_needs_runs_one_to_k() {
    let bundle = bundle(&two_labels(), 3);
    edit_json(&bundle.path().join("bundle.json"), |manifest| {
        let runs = manifest["runs"].as_array_mut().expect("runs");
        runs.retain(|run| !(run["label"] == "old" && run["run"] == 3));
    });
    edit_json(&bundle.path().join("host.json"), |host| {
        let runs = host["runs"].as_array_mut().expect("runs");
        runs.retain(|run| !(run["label"] == "old" && run["run"] == 3));
    });
    let error = bundle.error();
    assert!(error.contains("runs [1, 2] are not 1..=3"), "{error}");
}

#[test]
fn an_edited_cohort_is_refused() {
    let bundle = bundle(&two_labels(), 3);
    fs::write(
        bundle.path().join("cohorts/medium-set.txt"),
        "10001\n10003\n",
    )
    .expect("write");
    let error = bundle.error();
    assert!(error.contains("but the bundle records"), "{error}");
}

#[test]
fn a_narrowed_corpus_is_summarized_but_not_publishable() {
    let bundle = bundle(&two_labels(), 3);
    let list = "10001\n10002\n10003\n10004\n";
    fs::write(bundle.path().join("case-list.txt"), list).expect("write");
    edit_json(&bundle.path().join("bundle.json"), |manifest| {
        manifest["cases"]["case_list"] =
            json!({"path": "case-list.txt", "sha256": sha(list.as_bytes()), "source": "smoke"});
    });
    let summary = bundle.summarize().expect("summary");
    assert!(!summary.publishable);
    assert!(summary.publication_blockers[0].contains("narrowed to 4 cases"));

    // The case list must be exactly the cases the runner was given.
    let short = "10001\n10002\n";
    fs::write(bundle.path().join("case-list.txt"), short).expect("write");
    edit_json(&bundle.path().join("bundle.json"), |manifest| {
        manifest["cases"]["case_list"]["sha256"] = json!(sha(short.as_bytes()));
    });
    let error = bundle.error();
    assert!(error.contains("names 2 cases, not the 4 cases"), "{error}");
}

#[test]
fn builds_must_be_declared_release_and_alike_to_publish() {
    let bundle = bundle(&two_labels(), 3);
    for (label, flags) in [("old", ""), ("new", "-C target-cpu=native")] {
        edit_json(
            &bundle.path().join(label).join("build-contract.json"),
            |contract| {
                contract["target"]["build"]["rustflags"] = json!(flags);
            },
        );
        edit_json(&bundle.path().join(label).join("build.json"), |record| {
            record["rustflags"] = json!(flags);
        });
    }
    let summary = bundle.summarize().expect("summary");
    assert!(
        summary
            .publication_blockers
            .iter()
            .any(|blocker| blocker.contains("different RUSTFLAGS")),
        "{:?}",
        summary.publication_blockers
    );

    // A build record that disagrees with its contract is refused.
    edit_json(&bundle.path().join("new/build.json"), |record| {
        record["rustflags"] = json!("");
    });
    let error = bundle.error();
    assert!(error.contains("but the build contract declares"), "{error}");
}

#[test]
fn bundle_paths_stay_inside_the_bundle() {
    let bundle = bundle(&two_labels(), 3);
    edit_json(&bundle.path().join("bundle.json"), |manifest| {
        manifest["runs"][0]["raw"] = json!("../elsewhere/raw.jsonl");
    });
    edit_json(&bundle.path().join("host.json"), |_| {});
    let error = bundle.error();
    assert!(
        error.contains("is not a plain path inside the bundle"),
        "{error}"
    );
}

#[test]
fn case_lists_parse_strictly() {
    let ids = parse_case_list("# header\n00001  # P0 note\n42\n\n01127\n").expect("list");
    assert_eq!(
        ids.into_iter().collect::<Vec<_>>(),
        ["00001", "00042", "01127"]
    );
    for bad in ["00001\n00001\n", "123456\n", "abc\n", "# only comments\n"] {
        assert!(parse_case_list(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn a_binary_without_the_durability_knob_blocks_a_normal_durability_bundle() {
    let bundle = bundle(&two_labels(), 3);
    edit_json(&bundle.path().join("bundle.json"), |manifest| {
        manifest["labels"][0]["durability_env"] = json!(false);
    });
    let summary = bundle.summarize().expect("summary");
    assert_eq!(summary.labels[0].durability, "built-in default");
    assert_eq!(summary.labels[1].durability, "normal");
    assert!(
        summary
            .publication_blockers
            .iter()
            .any(|blocker| blocker
                .contains("old: the binary has no REDLINEDB_DEFAULT_DURABILITY knob")),
        "{:?}",
        summary.publication_blockers
    );

    // Leaving durability at every binary's built-in default is comparable.
    edit_json(&bundle.path().join("bundle.json"), |manifest| {
        manifest["protocol"]["durability"] = json!("default");
    });
    let summary = bundle.summarize().expect("summary");
    assert!(
        summary
            .labels
            .iter()
            .all(|label| label.durability == "built-in default")
    );
    assert!(summary.publishable, "{:?}", summary.publication_blockers);

    edit_json(&bundle.path().join("bundle.json"), |manifest| {
        manifest["protocol"]["durability"] = json!("strict");
    });
    let error = bundle.error();
    assert!(error.contains("not normal or default"), "{error}");
}

#[test]
fn temp_roots_off_tmpfs_block_publication() {
    let bundle = bundle(&two_labels(), 3);
    edit_json(&bundle.path().join("host.json"), |host| {
        host["tmp_filesystem"] = json!("ext2/ext3");
    });
    let summary = bundle.summarize().expect("summary");
    assert_eq!(summary.host.tmp_filesystem, "ext2/ext3");
    assert_eq!(
        summary.publication_blockers,
        ["the temp roots were on ext2/ext3, not tmpfs"]
    );
}
