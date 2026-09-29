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

/// A bundle of two versions and SQLite, three runs, two repetitions each,
/// where v2 is twice v1 and SQLite twice v2.
fn synthetic_bundle(root: &std::path::Path, runs: u32, divisor: u64) -> std::path::PathBuf {
    let bundle = root.join("bundle");
    std::fs::create_dir_all(&bundle).expect("bundle dir");
    std::fs::write(
        bundle.join("bundle.json"),
        serde_json::json!({
            "schema": crate::summary::BUNDLE_SCHEMA,
            "bundle": "synthetic",
            "labels": ["v1", "v2"],
            "rows": 100,
            "runs": runs,
            "pairs": ["normal"],
        })
        .to_string(),
    )
    .expect("bundle.json");
    for (label, elapsed_ns) in [("v1", 1_000_000_000u64), ("v2", 500_000_000)] {
        std::fs::create_dir_all(bundle.join(label)).expect("label dir");
        for run in 1..=runs {
            let mut lines = String::new();
            for rep in 0..2 {
                for (engine, elapsed, version) in [
                    (label, elapsed_ns, "redlinedb 5.1.1"),
                    ("sqlite", 250_000_000, "sqlite 3.50.2"),
                ] {
                    lines.push_str(
                        &serde_json::json!({
                            "status": "ok",
                            "label": engine,
                            "with": label,
                            "run": run,
                            "rep": rep,
                            "pair": "normal",
                            "workload": "point_pk_prepared",
                            "ops": 1_000,
                            "elapsed_ns": elapsed,
                            "digest": 42,
                            "work_divisor": divisor,
                            "engine_version": version,
                        })
                        .to_string(),
                    );
                    lines.push('\n');
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

#[test]
fn the_summary_pairs_versions_and_sqlite_run_by_run() {
    let root = tempfile::tempdir().expect("temp dir");
    let bundle = synthetic_bundle(root.path(), 3, 1);
    let summary = crate::summary::summarize(&bundle).expect("summary");
    assert!(summary.publishable, "{:?}", summary.blockers);
    assert_eq!(summary.sqlite_version.as_deref(), Some("sqlite 3.50.2"));
    let workload = &summary.workloads[0];
    assert_eq!(workload.throughput["v1"].median, Some(1_000.0));
    assert_eq!(workload.throughput["v2"].median, Some(2_000.0));
    assert_eq!(workload.throughput["sqlite"].median, Some(4_000.0));
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
    assert_eq!(
        crate::render::bundle_of(&block).as_deref(),
        Some("benchmark-results/perf/releases/synthetic")
    );
}

#[test]
fn a_bundle_with_too_few_runs_or_reduced_work_is_not_published() {
    let root = tempfile::tempdir().expect("temp dir");
    let bundle = synthetic_bundle(root.path(), 2, 50);
    let summary = crate::summary::summarize(&bundle).expect("summary");
    assert!(!summary.publishable);
    assert!(summary.blockers.iter().any(|b| b.contains("fewer than 3")));
    assert!(
        summary
            .blockers
            .iter()
            .any(|b| b.contains("work divisor 50"))
    );
    assert!(crate::render::block(&summary, "x").is_err());
}

#[test]
fn check_fails_when_summary_json_does_not_match_the_records() {
    let root = tempfile::tempdir().expect("temp dir");
    let bundle = synthetic_bundle(root.path(), 3, 1);
    crate::summary::write_or_check(&bundle, false).expect("write");
    crate::summary::write_or_check(&bundle, true).expect("fresh summary checks");
    let raw = bundle.join("v2").join("run-1-normal.jsonl");
    let text = std::fs::read_to_string(&raw).expect("read raw");
    std::fs::write(&raw, text.replace("500000000", "400000000")).expect("tamper");
    assert!(crate::summary::write_or_check(&bundle, true).is_err());
}
