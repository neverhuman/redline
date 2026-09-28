//! The version-history README block is rendered only from a publishable
//! bundle whose raw files are the ones it summarized, and only between
//! markers the README already has.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::*;

const README: &str = "# RedlineDB\n\nIntro.\n\n## Versions over time\n\n<!-- version-history:begin -->\nold table\n<!-- version-history:end -->\n\nOutro.\n";

/// A repository-shaped temp dir with a README and a two-version bundle at
/// benchmark-results/sqlite-parity/releases/v5.0.0.
struct Fixture {
    root: TempDir,
}

impl Fixture {
    fn new(runs: usize) -> Self {
        let root = TempDir::new().expect("tempdir");
        fs::write(root.path().join("README.md"), README).expect("write README");
        let bundle = root
            .path()
            .join("benchmark-results/sqlite-parity/releases/v5.0.0");
        let mut labels = Vec::new();
        for (index, (label, commit)) in [
            ("v4.1.0", "af20826311a067a51383c382013612545156df61"),
            ("v5.0.0", "0123456789abcdef0123456789abcdef01234567"),
        ]
        .into_iter()
        .enumerate()
        {
            let mut run_entries = Vec::new();
            for run in 1..=runs {
                let raw = format!("{label}/run-{run}/raw.jsonl");
                let path = bundle.join(&raw);
                fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
                let text =
                    format!("{{\"case_id\":\"00001\",\"label\":\"{label}\",\"run\":{run}}}\n");
                fs::write(&path, &text).expect("write raw");
                run_entries.push(json!({
                    "run": run, "raw": raw,
                    "raw_sha256": format!("{:x}", Sha256::digest(text.as_bytes()))
                }));
            }
            let (median, p95, delta) = if index == 0 {
                (
                    json!({"value": 3.1, "min": 3.0, "max": 3.2, "per_run": per_run(runs, [3.0, 3.1, 3.2])}),
                    json!({"value": 4.1, "min": 4.0, "max": 4.2, "per_run": per_run(runs, [4.0, 4.1, 4.2])}),
                    Value::Null,
                )
            } else {
                (
                    json!({"value": 1.6, "min": 1.5, "max": 1.7, "per_run": per_run(runs, [1.5, 1.6, 1.7])}),
                    json!({"value": 2.1, "min": 2.0, "max": 2.2, "per_run": per_run(runs, [2.0, 2.1, 2.2])}),
                    json!({
                        "previous": "v4.1.0",
                        "median_change_pct": (1.6 / 3.1 - 1.0) * 100.0,
                        "verdict": if runs < 2 { "unassessed" } else { "exceeds_noise" }
                    }),
                )
            };
            labels.push(json!({
                "label": label,
                "source_ref": label,
                "source_commit": commit,
                "binary_sha256": "binary",
                "version": format!("redlinedb {label}"),
                "build": {"declared": true, "profile": "release", "features": "default", "rustflags": ""},
                "durability": "built-in default",
                "build_rustc": "rustc 1.95.0",
                "pgo_training_corpus": null,
                "passed_cases": 2400 + index * 20,
                "flaky_cases": index,
                "runs": run_entries,
                "common": {"cases": 2390, "median": median, "p95": p95, "delta_vs_previous": delta},
                "medium": null
            }));
        }
        let publishable = runs >= 3;
        write_json(
            &bundle.join("summary.json"),
            &json!({
                "schema_version": "redline-release-bench-summary-v1",
                "bundle": "v5.0.0",
                "started_at_utc": "2026-10-01T08:00:00Z",
                "finished_at_utc": "2026-10-01T09:00:00Z",
                "estimator": "per case ratio of medians",
                "noise_rule": "ranges must not overlap",
                "protocol": {
                    "suite": "sqlite_parity", "workers": 1, "repetitions": 3, "warmup": 1,
                    "order": "alternate", "cpus": "2-5", "tmp_root": "/dev/shm/rl-bench",
                    "durability": "default", "measurement_boundary": "cli_case_wall_time",
                    "max_loadavg": 32.0, "command": "redline-testing run ..."
                },
                "host": {
                    "cpu_model": "Fixture CPU", "nproc": 128, "kernel": "Linux 6.8.0 x86_64",
                    "pinned_cpus": "2-5", "tmp_filesystem": "tmpfs", "governors": {},
                    "runner_units_active": [], "runs_with_runner_jobs": 0,
                    "max_loadavg_threshold": 32.0, "max_loadavg_observed": 3.5
                },
                "runner": {"path": "/r", "sha256": "runnersha", "version": "redline-testing 1.0.1"},
                "reference": {
                    "path": "/s", "sha256": "fd3bdd25217a849f8f4fa295",
                    "version": "3.53.1 2026-05-05 10:34:17 c88b (64-bit)"
                },
                "corpus": {"cases": 2445, "narrowed_by": null},
                "runs_per_label": runs,
                "common_pass_set": {"definition": "every label, every run", "cases": 2390},
                "medium_cohort": null,
                "labels": labels,
                "publishable": publishable,
                "publication_blockers": if publishable {
                    Vec::<String>::new()
                } else {
                    vec![format!("{runs} run(s) per label; a spread needs at least 3")]
                }
            }),
        );
        Self { root }
    }

    fn readme(&self) -> std::path::PathBuf {
        self.root.path().join("README.md")
    }

    fn bundle(&self) -> std::path::PathBuf {
        self.root
            .path()
            .join("benchmark-results/sqlite-parity/releases/v5.0.0")
    }

    fn options(&self, check: bool) -> VersionHistoryOptions {
        VersionHistoryOptions {
            bundle: self.bundle(),
            readme: Some(self.readme()),
            check,
        }
    }

    fn edit_summary(&self, edit: impl FnOnce(&mut Value)) {
        let path = self.bundle().join("summary.json");
        let mut value: Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("json");
        edit(&mut value);
        write_json(&path, &value);
    }
}

fn per_run(runs: usize, values: [f64; 3]) -> Vec<f64> {
    if runs == 1 {
        vec![values[1]]
    } else {
        values[..runs.min(3)].to_vec()
    }
}

/// One way to break a summary.
type Breakage = (&'static str, fn(&mut Value));

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(value).expect("json")),
    )
    .expect("write");
}

fn block(readme: &str) -> &str {
    let start = readme.find(BEGIN).expect("begin") + BEGIN.len();
    &readme[start..readme.find(END).expect("end")]
}

#[test]
fn renders_the_table_between_the_markers_and_nothing_else() {
    let fixture = Fixture::new(3);
    run(fixture.options(false)).expect("render");
    let readme = fs::read_to_string(fixture.readme()).expect("read");
    assert!(readme.starts_with("# RedlineDB\n\nIntro.\n\n## Versions over time\n\n"));
    assert!(readme.ends_with("<!-- version-history:end -->\n\nOutro.\n"));
    let block = block(&readme);
    assert!(!block.contains("old table"));
    assert!(block.contains(
        "| Version | Commit | SQLite corpus passed (of 2445, today's corpus) | Median latency ratio vs SQLite (lower is better) | p95 | Δ median vs previous |"
    ), "{block}");
    assert!(
        block.contains(
            "| v4.1.0 | `af2082631` | 2400 | 3.100× (3.000–3.200) | 4.100× (4.000–4.200) | — |"
        ),
        "{block}"
    );
    assert!(
        block.contains("| v5.0.0 | `012345678` | 2420 (+1 flaky) | 1.600× (1.500–1.700) | 2.100× (2.000–2.200) | -48.4% |"),
        "{block}"
    );
    for fragment in [
        "per-case CLI process wall time (`cli_case_wall_time`)",
        "the 2390 cases every version passed in every run",
        "Each version ran 3 time(s), interleaved",
        "Measured 2026-10-01 on Fixture CPU (128 CPUs, Linux 6.8.0 x86_64), pinned to CPUs 2-5, 1 worker(s), `--order alternate`, temp roots on tmpfs, each version's built-in default durability.",
        "SQLite reference 3.53.1 (`fd3bdd25217a`); runner redline-testing 1.0.1.",
        "RUSTFLAGS `\"\"`, without PGO.",
        "Bundle: [`benchmark-results/sqlite-parity/releases/v5.0.0`](benchmark-results/sqlite-parity/releases/v5.0.0/).",
    ] {
        assert!(block.contains(fragment), "missing {fragment:?} in\n{block}");
    }
    assert!(!block.contains("Not publishable"));

    // --check accepts what it just wrote and refuses a hand edit.
    run(fixture.options(true)).expect("an unchanged block checks");
    fs::write(fixture.readme(), readme.replace("-48.4%", "-50.0%")).expect("edit");
    let error = format!("{:#}", run(fixture.options(true)).expect_err("drift"));
    assert!(
        error.contains("is not what bundle v5.0.0 renders"),
        "{error}"
    );
}

#[test]
fn a_change_inside_the_noise_reads_within_noise() {
    let fixture = Fixture::new(3);
    fixture.edit_summary(|summary| {
        summary["labels"][1]["common"]["delta_vs_previous"]["verdict"] = json!("within_noise");
    });
    run(fixture.options(false)).expect("render");
    let readme = fs::read_to_string(fixture.readme()).expect("read");
    assert!(
        readme.contains("| 2.100× (2.000–2.200) | within noise |"),
        "{readme}"
    );
}

#[test]
fn a_readme_without_the_markers_is_refused_and_left_alone() {
    let fixture = Fixture::new(3);
    for text in [
        "# RedlineDB\n\nNo table yet.\n".to_owned(),
        format!("{BEGIN}\n{END}\n{BEGIN}\n{END}\n"),
        format!("{END}\n{BEGIN}\n"),
    ] {
        fs::write(fixture.readme(), &text).expect("write");
        assert!(run(fixture.options(false)).is_err(), "{text:?}");
        assert_eq!(fs::read_to_string(fixture.readme()).expect("read"), text);
    }
}

#[test]
fn a_bundle_that_is_not_publishable_never_reaches_the_readme() {
    let fixture = Fixture::new(1);
    let error = format!("{:#}", run(fixture.options(false)).expect_err("K=1"));
    assert!(
        error.contains("not publishable") && error.contains("1 run(s) per label"),
        "{error}"
    );
    assert_eq!(fs::read_to_string(fixture.readme()).expect("read"), README);

    // Printed without --readme, it says so and shows no ranges.
    let summary = load_summary(&fixture.bundle()).expect("summary");
    let block = render_block(&summary, "bundle").expect("render");
    assert!(
        block.contains("> **Not publishable:** 1 run(s) per label"),
        "{block}"
    );
    assert!(block.contains("(of 2445, today's corpus)"), "{block}");
    assert!(
        block.contains("| 1.600× | 2.100× | not assessed (1 run) |"),
        "{block}"
    );
}

#[test]
fn a_narrowed_bundle_does_not_claim_the_corpus() {
    let fixture = Fixture::new(3);
    fixture.edit_summary(|summary| {
        summary["corpus"] = json!({"cases": 20, "narrowed_by": {"path": "case-list.txt"}});
        summary["publishable"] = json!(false);
        summary["publication_blockers"] = json!(["the corpus is narrowed to 20 cases"]);
    });
    let summary = load_summary(&fixture.bundle()).expect("summary");
    let block = render_block(&summary, "bundle").expect("render");
    assert!(
        block.contains("SQLite corpus passed (of 20 listed cases)"),
        "{block}"
    );
    assert!(!block.contains("today's corpus"), "{block}");
    assert!(
        block.contains("The corpus was narrowed by a case list."),
        "{block}"
    );
}

#[test]
fn a_raw_file_changed_after_the_summary_is_refused() {
    let fixture = Fixture::new(3);
    fs::write(fixture.bundle().join("v5.0.0/run-2/raw.jsonl"), "{}\n").expect("tamper");
    let error = format!("{:#}", run(fixture.options(false)).expect_err("tampered"));
    assert!(
        error.contains("not the") && error.contains("rerun perf_evidence summarize-bundle"),
        "{error}"
    );
    assert_eq!(fs::read_to_string(fixture.readme()).expect("read"), README);
}

#[test]
fn malformed_summaries_are_refused() {
    let cases: [Breakage; 5] = [
        ("schema", |summary| {
            summary["schema_version"] = json!("redline-release-bench-summary-v0");
        }),
        ("label", |summary| {
            summary["labels"][0]["label"] = json!("v4 | injected");
        }),
        ("commit", |summary| {
            summary["labels"][0]["source_commit"] = json!("`rm`");
        }),
        ("runs", |summary| {
            summary["runs_per_label"] = json!(4);
        }),
        ("path", |summary| {
            summary["labels"][0]["runs"][0]["raw"] = json!("../outside.jsonl");
        }),
    ];
    for (name, edit) in cases {
        let fixture = Fixture::new(3);
        fixture.edit_summary(edit);
        assert!(run(fixture.options(false)).is_err(), "{name}");
        assert_eq!(
            fs::read_to_string(fixture.readme()).expect("read"),
            README,
            "{name}"
        );
    }
}

#[test]
fn the_bundle_must_live_under_the_readme() {
    let fixture = Fixture::new(3);
    let elsewhere = TempDir::new().expect("tempdir");
    fs::write(elsewhere.path().join("README.md"), README).expect("write");
    let error = format!(
        "{:#}",
        run(VersionHistoryOptions {
            bundle: fixture.bundle(),
            readme: Some(elsewhere.path().join("README.md")),
            check: false,
        })
        .expect_err("outside")
    );
    assert!(error.contains("is not inside"), "{error}");
}

#[test]
fn check_needs_a_readme() {
    let fixture = Fixture::new(3);
    let error = run(VersionHistoryOptions {
        bundle: fixture.bundle(),
        readme: None,
        check: true,
    })
    .expect_err("check without readme");
    assert!(format!("{error:#}").contains("give --readme"));
}
