//! The harness itself: both engines run every workload on the same data and
//! agree on what they read or leave behind, and each opens with the
//! settings its pair names.

use crate::case::{self, Measured};
use crate::image::ImageKind;
use crate::pair::{Engine, Pair};
use crate::workloads::CATALOG;

const ROWS: u64 = 300;
/// Enough work to exercise every statement without a slow debug test.
const DIVISOR: u64 = 50;

fn images(root: &std::path::Path, engine: Engine) -> (std::path::PathBuf, std::path::PathBuf) {
    let base = root.join(format!("{engine}-base"));
    let after = root.join(format!("{engine}-after"));
    case::build_image(engine, ImageKind::Base, ROWS, &base, None).expect("base image");
    case::build_image(engine, ImageKind::AfterUpdates, ROWS, &after, Some(&base))
        .expect("after-updates image");
    (base, after)
}

#[test]
fn both_engines_agree_on_every_workload() {
    let root = tempfile::tempdir().expect("temp dir");
    let redline = images(root.path(), Engine::Redline);
    let sqlite = images(root.path(), Engine::Sqlite);
    for workload in CATALOG {
        let mut measured: Vec<Measured> = Vec::new();
        for (engine, (base, after)) in [(Engine::Redline, &redline), (Engine::Sqlite, &sqlite)] {
            let image = match workload.image {
                ImageKind::Base => base,
                ImageKind::AfterUpdates => after,
            };
            let work = root.path().join(format!("work-{engine}-{}", workload.id));
            let record =
                case::run_case(engine, workload, ROWS, Pair::Normal, DIVISOR, image, &work)
                    .unwrap_or_else(|err| panic!("{engine} {}: {err:#}", workload.id));
            assert!(record.ops > 0, "{engine} {} did no work", workload.id);
            assert!(
                record.elapsed_ns > 0,
                "{engine} {} took no time",
                workload.id
            );
            assert_eq!(record.work_divisor, DIVISOR);
            measured.push(record);
        }
        assert_eq!(
            measured[0].digest, measured[1].digest,
            "{}: redline and sqlite disagree on the result",
            workload.id
        );
        assert_eq!(
            measured[0].ops, measured[1].ops,
            "{}: unequal work",
            workload.id
        );
    }
}

#[test]
fn each_engine_reports_the_settings_of_its_pair() {
    let root = tempfile::tempdir().expect("temp dir");
    for pair in [Pair::Normal, Pair::Strict] {
        let path = root.path().join(format!("redline-{pair}"));
        let redline = crate::redline::Redline::open(&path, pair).expect("open redline");
        let settings = crate::driver::Driver::settings(&redline).expect("settings");
        let durability = match pair {
            Pair::Normal => "Normal",
            Pair::Strict => "Strict",
        };
        assert_eq!(settings["durability"], durability);
        assert!(
            settings["query_pool_threads"]
                .as_u64()
                .is_some_and(|n| n <= 1)
        );

        let path = root.path().join(format!("sqlite-{pair}.db"));
        let sqlite = crate::sqlite::Sqlite::open(&path, pair).expect("open sqlite");
        let settings = crate::driver::Driver::settings(&sqlite).expect("settings");
        assert_eq!(settings["journal_mode"], "wal");
        assert_eq!(settings["cache_size"], -65536);
        assert_eq!(
            settings["synchronous"],
            match pair {
                Pair::Normal => 1,
                Pair::Strict => 2,
            }
        );
    }
}

#[test]
fn a_record_survives_a_json_round_trip() {
    let root = tempfile::tempdir().expect("temp dir");
    let (base, _) = images(root.path(), Engine::Sqlite);
    let workload = crate::workloads::find("point_pk_prepared").expect("workload");
    let record = case::run_case(
        Engine::Sqlite,
        &workload,
        ROWS,
        Pair::Normal,
        DIVISOR,
        &base,
        &root.path().join("work"),
    )
    .expect("case");
    let line = serde_json::to_string(&record).expect("encode");
    let back: Measured = serde_json::from_str(&line).expect("decode");
    assert_eq!(back.schema, case::RECORD_SCHEMA);
    assert_eq!(back.digest, record.digest);
    assert_eq!(back.workload, "point_pk_prepared");
}

#[test]
fn the_readme_block_matches_its_bundle() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let readme = root.join("README.md");
    let text = std::fs::read_to_string(&readme).expect("read README.md");
    // Before the first bundle is published the README has no block.
    let Some(bundle) = crate::render::bundle_of(&text) else {
        return;
    };
    let summary = crate::summary::write_or_check(&root.join(&bundle), true)
        .expect("summary.json matches the bundle's raw records");
    crate::render::render_into(&summary, &bundle, &readme, true)
        .expect("the README block matches the bundle");
}

/// Knobs for a synthetic bundle; `Default` is a clean, publishable one.
#[derive(Clone, Copy)]
struct Synthetic {
    runs: u32,
    divisor: u64,
    /// Seconds SQLite takes beside v1 (beside v2 it takes 0.25 s).
    sqlite_beside_v1: f64,
    /// Make one v2 repetition time out.
    timeout: bool,
    /// Give one v2 record a different digest.
    wrong_digest: bool,
    /// Record a CI job during v2's first run.
    busy_host: bool,
    /// Scale written in the records (the manifest says 100).
    record_rows: u64,
    /// Seconds v2 takes (v1 takes 1.0).
    v2_seconds: f64,
    /// Record that v2's first run started on a busy host.
    waited_out: bool,
    /// A workload v2 did not measure.
    v2_skips: Option<&'static str>,
    /// The file system host.json names for the normal pair.
    normal_fs: &'static str,
    /// The engine version v2's records name in its third run.
    v2_run3_version: &'static str,
    /// Repetitions written per run (the manifest asks for 2).
    reps_written: u32,
    /// The highest load recorded for v2's first run (the limit is 16).
    v2_run1_load_max: f64,
    /// Write one of v1's records into v2's file.
    misfile: bool,
    /// Leave v2's second run out of runs.jsonl.
    drop_host_record: bool,
    /// Seconds SQLite takes beside v1 in run 3 (otherwise as in other runs).
    sqlite_v1_run3_seconds: Option<f64>,
}

impl Default for Synthetic {
    fn default() -> Self {
        Self {
            runs: 3,
            divisor: 1,
            sqlite_beside_v1: 0.25,
            timeout: false,
            wrong_digest: false,
            busy_host: false,
            record_rows: 100,
            v2_seconds: 0.5,
            waited_out: false,
            v2_skips: None,
            normal_fs: "tmpfs",
            v2_run3_version: "redlinedb 5.1.1",
            reps_written: 2,
            v2_run1_load_max: 2.0,
            misfile: false,
            drop_host_record: false,
            sqlite_v1_run3_seconds: None,
        }
    }
}

/// A bundle of two versions and SQLite beside each, every normal-pair
/// workload, two repetitions: v2 is twice v1 and SQLite twice v2.
fn synthetic_bundle(root: &std::path::Path, knobs: Synthetic) -> std::path::PathBuf {
    let bundle = root.join("bundle");
    std::fs::create_dir_all(&bundle).expect("bundle dir");
    let write = |name: &str, value: serde_json::Value| {
        std::fs::write(bundle.join(name), value.to_string()).expect("write bundle file")
    };
    write(
        "bundle.json",
        serde_json::json!({
            "schema": crate::summary::BUNDLE_SCHEMA,
            "bundle": "synthetic",
            "labels": ["v1", "v2"],
            "rows": 100,
            "runs": knobs.runs,
            "reps": 2,
            "pairs": ["normal"],
        }),
    );
    write(
        "host.json",
        serde_json::json!({ "normal_dir_fs": knobs.normal_fs }),
    );
    let mut runs = String::new();
    for label in ["v1", "v2"] {
        for run in 1..=knobs.runs {
            if knobs.drop_host_record && label == "v2" && run == 2 {
                continue;
            }
            let busy = u64::from(knobs.busy_host && label == "v2" && run == 1);
            let waited_out = knobs.waited_out && label == "v2" && run == 1;
            let load_max = if label == "v2" && run == 1 {
                knobs.v2_run1_load_max
            } else {
                2.0
            };
            runs.push_str(
                &serde_json::json!({
                    "label": label, "run": run, "pair": "normal", "waited_out": waited_out,
                    "runner_jobs_before": 0, "runner_jobs_after": 0, "runner_jobs_max": busy,
                    "loadavg_max": load_max, "max_loadavg": 16.0,
                })
                .to_string(),
            );
            runs.push('\n');
        }
    }
    std::fs::write(bundle.join("runs.jsonl"), runs).expect("runs.jsonl");
    let workloads = crate::run::selected(&None, Pair::Normal).expect("workloads");
    for (label, own_s) in [("v1", 1.0f64), ("v2", knobs.v2_seconds)] {
        std::fs::create_dir_all(bundle.join(label)).expect("label dir");
        for run in 1..=knobs.runs {
            let mut lines = String::new();
            for workload in &workloads {
                if label == "v2" && knobs.v2_skips == Some(workload.id) {
                    continue;
                }
                for rep in 0..knobs.reps_written {
                    let sqlite_s = match (label, run, knobs.sqlite_v1_run3_seconds) {
                        ("v1", 3, Some(seconds)) => seconds,
                        ("v1", _, _) => knobs.sqlite_beside_v1,
                        _ => 0.25,
                    };
                    let own_version = if label == "v2" && run == 3 {
                        knobs.v2_run3_version
                    } else {
                        "redlinedb 5.1.1"
                    };
                    for (engine, seconds, version) in [
                        (label, own_s, own_version),
                        ("sqlite", sqlite_s, "sqlite 3.50.2"),
                    ] {
                        let odd = engine == "v2" && run == 1 && rep == 0 && workload.id == "top10";
                        let status = if knobs.timeout && odd {
                            "timeout"
                        } else {
                            "ok"
                        };
                        let digest = if knobs.wrong_digest && odd { 43 } else { 42 };
                        lines.push_str(
                            &serde_json::json!({
                                "status": status,
                                "label": engine,
                                "with": if knobs.misfile && label == "v2" && run == 1 && rep == 0 {
                                    "v1"
                                } else {
                                    label
                                },
                                "run": run,
                                "rep": rep,
                                "pair": "normal",
                                "rows": knobs.record_rows,
                                "workload": workload.id,
                                "ops": 1_000,
                                "elapsed_ns": (seconds * 1e9) as u64,
                                "digest": digest,
                                "work_divisor": knobs.divisor,
                                "engine_version": version,
                            })
                            .to_string(),
                        );
                        lines.push('\n');
                    }
                }
            }
            std::fs::write(
                bundle.join(label).join(format!("run-{run}-normal.jsonl")),
                lines,
            )
            .expect("raw records");
        }
    }
    bundle
}

fn blockers_of(knobs: Synthetic) -> Vec<String> {
    let root = tempfile::tempdir().expect("temp dir");
    let bundle = synthetic_bundle(root.path(), knobs);
    let summary = crate::summary::summarize(&bundle).expect("summary");
    assert!(!summary.publishable, "expected blockers");
    assert!(crate::render::block(&summary, "x").is_err());
    summary.blockers
}

#[test]
fn the_summary_pairs_versions_and_sqlite_run_by_run() {
    let root = tempfile::tempdir().expect("temp dir");
    let bundle = synthetic_bundle(root.path(), Synthetic::default());
    let summary = crate::summary::summarize(&bundle).expect("summary");
    assert!(summary.publishable, "{:?}", summary.blockers);
    assert_eq!(summary.sqlite_version.as_deref(), Some("sqlite 3.50.2"));
    let workload = summary
        .workloads
        .iter()
        .find(|w| w.id == "point_pk_prepared")
        .expect("workload");
    assert_eq!(workload.throughput["v1"].median, Some(1_000.0));
    assert_eq!(workload.throughput["v2"].median, Some(2_000.0));
    assert_eq!(workload.throughput["sqlite@v2"].median, Some(4_000.0));
    let speedup = &workload.speedup["v2/v1"];
    assert_eq!(speedup.median, Some(2.0));
    assert!(speedup.exceeds_noise);
    assert_eq!(workload.vs_sqlite["v2"].median, Some(0.5));
    assert!(workload.digest_agrees);

    let block = crate::render::block(&summary, "benchmark-results/perf/releases/synthetic")
        .expect("render");
    assert!(
        block.contains(
            "| SELECT by INTEGER PRIMARY KEY, prepared | 1.00k stmt/s | 2.00k stmt/s | 2.00× | 4.00k stmt/s | 0.50 |"
        ),
        "{block}"
    );
    assert!(block.contains("median of 2 repetitions"), "{block}");
    assert_eq!(
        crate::render::bundle_of(&block).as_deref(),
        Some("benchmark-results/perf/releases/synthetic")
    );
}

#[test]
fn too_few_runs_or_reduced_work_block_publication() {
    let blockers = blockers_of(Synthetic {
        runs: 2,
        divisor: 50,
        ..Synthetic::default()
    });
    assert!(
        blockers.iter().any(|b| b.contains("fewer than 3")),
        "{blockers:?}"
    );
    assert!(
        blockers.iter().any(|b| b.contains("work divisor 50")),
        "{blockers:?}"
    );
}

#[test]
fn a_timed_out_repetition_blocks_publication() {
    let blockers = blockers_of(Synthetic {
        timeout: true,
        ..Synthetic::default()
    });
    assert!(
        blockers.iter().any(|b| b.contains("top10 run 1: timeout")),
        "{blockers:?}"
    );
}

#[test]
fn disagreeing_results_block_publication() {
    let blockers = blockers_of(Synthetic {
        wrong_digest: true,
        ..Synthetic::default()
    });
    assert!(
        blockers.iter().any(|b| b.contains("disagree")),
        "{blockers:?}"
    );
}

#[test]
fn sqlite_drifting_between_versions_blocks_publication() {
    let blockers = blockers_of(Synthetic {
        sqlite_beside_v1: 0.30,
        ..Synthetic::default()
    });
    assert!(
        blockers.iter().any(|b| b.contains("not steady")),
        "{blockers:?}"
    );
}

#[test]
fn a_ci_job_during_a_run_blocks_publication() {
    let blockers = blockers_of(Synthetic {
        busy_host: true,
        ..Synthetic::default()
    });
    assert!(
        blockers.iter().any(|b| b.contains("CI job")),
        "{blockers:?}"
    );
}

#[test]
fn records_of_another_scale_block_publication() {
    let blockers = blockers_of(Synthetic {
        record_rows: 500,
        ..Synthetic::default()
    });
    assert!(
        blockers.iter().any(|b| b.contains("measured at 500 rows")),
        "{blockers:?}"
    );
}

#[test]
fn check_fails_when_summary_json_does_not_match_the_records() {
    let root = tempfile::tempdir().expect("temp dir");
    let bundle = synthetic_bundle(root.path(), Synthetic::default());
    crate::summary::write_or_check(&bundle, false).expect("write");
    crate::summary::write_or_check(&bundle, true).expect("fresh summary checks");
    let raw = bundle.join("v2").join("run-1-normal.jsonl");
    let text = std::fs::read_to_string(&raw).expect("read raw");
    std::fs::write(&raw, text.replace("500000000", "400000000")).expect("tamper");
    assert!(crate::summary::write_or_check(&bundle, true).is_err());
}

#[test]
fn the_after_updates_image_keeps_each_engines_log() {
    let root = tempfile::tempdir().expect("temp dir");
    let (base, after) = images(root.path(), Engine::Sqlite);
    let wal = |dir: &std::path::Path| {
        std::fs::metadata(dir.join("db.sqlite-wal")).map_or(0, |meta| meta.len())
    };
    assert_eq!(wal(&base), 0, "the base image is checkpointed");
    assert!(
        wal(&after) > 0,
        "the after-updates image keeps SQLite's log"
    );
}

#[test]
fn a_run_started_on_a_busy_host_blocks_publication() {
    let blockers = blockers_of(Synthetic {
        waited_out: true,
        ..Synthetic::default()
    });
    assert!(
        blockers
            .iter()
            .any(|b| b.contains("started on a busy host")),
        "{blockers:?}"
    );
}

#[test]
fn a_version_that_skipped_a_workload_blocks_publication() {
    let blockers = blockers_of(Synthetic {
        v2_skips: Some("top10"),
        ..Synthetic::default()
    });
    assert!(
        blockers
            .iter()
            .any(|b| b.contains("v2 normal top10: 0 of 3 runs measured")),
        "{blockers:?}"
    );
}

#[test]
fn a_normal_pair_off_tmpfs_blocks_publication() {
    let blockers = blockers_of(Synthetic {
        normal_fs: "ext4",
        ..Synthetic::default()
    });
    assert!(
        blockers.iter().any(|b| b.contains("ran on ext4")),
        "{blockers:?}"
    );
}

#[test]
fn a_series_that_ran_two_engine_versions_blocks_publication() {
    let blockers = blockers_of(Synthetic {
        v2_run3_version: "redlinedb 5.1.2",
        ..Synthetic::default()
    });
    assert!(
        blockers
            .iter()
            .any(|b| b.contains("several engine versions")),
        "{blockers:?}"
    );
}

#[test]
fn too_few_repetitions_block_publication() {
    let root = tempfile::tempdir().expect("temp dir");
    let knobs = Synthetic {
        reps_written: 1,
        ..Synthetic::default()
    };
    let bundle = synthetic_bundle(root.path(), knobs);
    // The manifest asks for 3 repetitions; the records hold 1.
    let manifest = bundle.join("bundle.json");
    let text = std::fs::read_to_string(&manifest).expect("read manifest");
    std::fs::write(&manifest, text.replace("\"reps\":2", "\"reps\":3")).expect("manifest");
    let summary = crate::summary::summarize(&bundle).expect("summary");
    assert!(
        summary
            .blockers
            .iter()
            .any(|b| b.contains("1 repetitions, fewer than 3")),
        "{:?}",
        summary.blockers
    );
}

#[test]
fn a_small_change_is_not_called_beyond_noise() {
    let root = tempfile::tempdir().expect("temp dir");
    let bundle = synthetic_bundle(
        root.path(),
        Synthetic {
            v2_seconds: 1.0 / 1.03,
            ..Synthetic::default()
        },
    );
    let summary = crate::summary::summarize(&bundle).expect("summary");
    assert!(summary.publishable, "{:?}", summary.blockers);
    let speedup = &summary.workloads[0].speedup["v2/v1"];
    assert!((speedup.median.expect("median") - 1.03).abs() < 1e-6);
    assert!(!speedup.exceeds_noise, "3% is under the 5% floor");
}

#[test]
fn high_host_load_during_a_run_blocks_publication() {
    let blockers = blockers_of(Synthetic {
        v2_run1_load_max: 20.0,
        ..Synthetic::default()
    });
    assert!(
        blockers.iter().any(|b| b.contains("host load reached 20")),
        "{blockers:?}"
    );
}

#[test]
fn a_record_in_another_versions_file_blocks_publication() {
    let blockers = blockers_of(Synthetic {
        misfile: true,
        ..Synthetic::default()
    });
    assert!(
        blockers.iter().any(|b| b.contains("in v2's file")),
        "{blockers:?}"
    );
}

#[test]
fn a_run_missing_from_the_host_record_blocks_publication() {
    let blockers = blockers_of(Synthetic {
        drop_host_record: true,
        ..Synthetic::default()
    });
    assert!(
        blockers
            .iter()
            .any(|b| b.contains("v2 run 2 normal: no host record")),
        "{blockers:?}"
    );
}

#[test]
fn sqlite_spread_beside_the_baseline_widens_the_noise_floor() {
    let root = tempfile::tempdir().expect("temp dir");
    // SQLite beside v1 takes 20% longer in one of three runs; its median,
    // and so the drift check, is unchanged, but its throughput spreads 17%.
    let bundle = synthetic_bundle(
        root.path(),
        Synthetic {
            v2_seconds: 1.0 / 1.12,
            sqlite_v1_run3_seconds: Some(0.3),
            ..Synthetic::default()
        },
    );
    let summary = crate::summary::summarize(&bundle).expect("summary");
    assert!(summary.publishable, "{:?}", summary.blockers);
    let speedup = &summary.workloads[0].speedup["v2/v1"];
    assert!((speedup.median.expect("median") - 1.12).abs() < 1e-6);
    assert!(
        !speedup.exceeds_noise,
        "12% is inside SQLite's 17% spread beside v1"
    );
}
