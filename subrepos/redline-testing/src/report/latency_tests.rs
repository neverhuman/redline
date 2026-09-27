//! Latency-ratio regression tests for the `report` renderer (L-04/R4-01).
//!
//! These drive `generate()` end to end and read back the committed-artifact
//! shapes (ranked.csv, README block), so they pin what readers actually see.

use std::fs;
use std::path::{Path, PathBuf};

use super::generate;
use super::ratio::summarize;
use super::render::rank_cases;
use super::types::{RawRecord, ReportOptions};
use super::utils::sha256_hex;
use crate::latency::RANKED_CSV_HEADER;

fn temp_root(name: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("redline-testing-{name}-"))
        .tempdir()
        .expect("temp root")
}

/// One passed case measured three times with fixed reference/target timings.
fn raw_case(case_id: &str, name: &str, reference_ns: u128, target_ns: u128) -> String {
    let mut out = String::new();
    for repetition in 1..=3usize {
        let record = serde_json::json!({
            "case_id": case_id,
            "name": name,
            "case_file": format!("{name}.sql"),
            "priority": "P0",
            "profile": "memory",
            "category": "SMOKE",
            "sample_role": format!("measured:{repetition}"),
            "repetition_index": repetition,
            "status": "passed",
            "reference_elapsed_ns": reference_ns,
            "target_elapsed_ns": target_ns,
        });
        out.push_str(&serde_json::to_string(&record).expect("raw json"));
        out.push('\n');
    }
    out
}

fn write_processed_evidence(root: &Path, suite: &str, raw_text: &str) -> PathBuf {
    let evidence = root.join("official-evidence.processed.json");
    let value = serde_json::json!({
        "schema_version": "redline-testing-official-evidence-processed-v1",
        "command_line": ["/ci/bin/redline-testing", "run", "--suite", "all", "--workers", "auto"],
        "official_evidence": {
            "schema_version": "redline-testing-official-evidence-v1",
            "runner": { "version": "redline-testing 1.0.1" },
            "target": { "version": "redlinedb test" },
            "sqlite": { "version": "3.53.1 test" },
            "command_line": [
                "/ci/bin/redline-testing", "run", "--suite", "all", "--workers", "auto",
                "--repetitions", "3", "--warmup", "0"
            ],
            "workers": "128",
            "repetitions": 3,
            "warmup": 0,
        },
        "suite_summaries": { suite: { "raw_sha256": sha256_hex(raw_text) } },
    });
    fs::write(
        &evidence,
        serde_json::to_string_pretty(&value).expect("evidence json"),
    )
    .expect("write evidence");
    evidence
}

struct Generated {
    ranked_csv: String,
    summary_json: String,
    readme: String,
    svgs: Vec<String>,
    _root: tempfile::TempDir,
}

fn try_generate(suite: &str, raw_text: &str) -> anyhow::Result<Generated> {
    let root = temp_root("latency-report");
    let input = root.path().join("raw.jsonl");
    fs::write(&input, raw_text).expect("raw");
    let evidence = write_processed_evidence(root.path(), suite, raw_text);
    let readme = root.path().join("README.md");
    fs::write(
        &readme,
        "# Report\n\n<!-- sqlite-parity-report:begin -->\n<!-- sqlite-parity-report:end -->\n",
    )
    .expect("readme");
    let out_dir = root.path().join("out");
    let svg = |name: &str| Some(root.path().join(name));
    let svg_paths = ["latency.svg", "histogram.svg", "median.svg"];
    generate(ReportOptions {
        suite: suite.to_owned(),
        input,
        official_evidence: Some(evidence),
        local_diagnostics: false,
        out_dir: out_dir.clone(),
        readme: readme.clone(),
        plot: svg(svg_paths[0]),
        ksloc_plot: None,
        performance_histogram_plot: svg(svg_paths[1]),
        median_test_performance_plot: svg(svg_paths[2]),
        jankurai_score: None,
        jankurai_comparison: None,
        jankurai_comparison_plot: None,
        jankurai_score_plot: None,
        code_shape_plot: None,
        updated_date: "2026-09-24".to_owned(),
        expected_repetitions: Some(3),
        expected_warmup: Some(0),
        check: false,
    })?;
    let names = super::evidence::artifact_names_for_suite(suite);
    let read = |path: PathBuf| fs::read_to_string(&path).expect("generated artifact");
    Ok(Generated {
        ranked_csv: read(out_dir.join(names.ranked)),
        summary_json: read(out_dir.join(names.summary)),
        readme: read(readme),
        svgs: svg_paths
            .iter()
            .map(|name| read(root.path().join(name)))
            .collect(),
        _root: root,
    })
}

fn generate_report(raw_text: &str) -> Generated {
    try_generate("sqlite_parity", raw_text).expect("report should generate")
}

fn csv_row<'a>(ranked_csv: &'a str, case_id: &str) -> &'a str {
    ranked_csv
        .lines()
        .find(|line| line.split(',').nth(1) == Some(case_id))
        .unwrap_or_else(|| panic!("no ranked.csv row for {case_id}:\n{ranked_csv}"))
}

/// A 1 ms SQLite case that RedlineDB runs in 2 ms is twice as slow. The old
/// 3 ms reference floor turned it into a +33.33% "improvement" and a win.
#[test]
fn improvement_is_raw_ratio_without_floor() {
    let generated = generate_report(&raw_case("00001", "SUB_MS", 1_000_000, 2_000_000));
    let row = csv_row(&generated.ranked_csv, "00001");
    assert!(
        !row.contains("33.333333"),
        "3 ms floor still applied: {row}"
    );
    assert!(row.contains(",2.000000,"), "latency ratio missing: {row}");
    assert!(row.contains(",-100.000000,"), "raw gap missing: {row}");
    assert!(
        generated.readme.contains("faster **0/1**"),
        "a 2x slower case was reported as faster:\n{}",
        generated.readme
    );
}

/// README row 11 of the 2026-09-24 block: the floor reported -20327.51%.
#[test]
fn trigger_row_ratio_is_unfloored() {
    let generated = generate_report(&raw_case(
        "00011",
        "INSTEAD_OF_TRIGGER_ON_VIEW",
        2_916_608,
        612_825_241,
    ));
    let row = csv_row(&generated.ranked_csv, "00011");
    assert!(row.contains(",210.115738,"), "ratio: {row}");
    assert!(row.contains(",-20911.573753,"), "raw gap: {row}");
    assert!(!row.contains("-20327.508033"), "floored gap: {row}");
}

fn three_cases() -> String {
    format!(
        "{}{}{}",
        raw_case("00001", "FASTER", 10_000_000, 5_000_000),
        raw_case("00002", "SUB_MS", 1_000_000, 2_000_000),
        raw_case("00003", "SLOW", 4_000_000, 40_000_000),
    )
}

#[test]
fn ranked_csv_is_sorted_by_ratio_with_versioned_header() {
    let generated = generate_report(&three_cases());
    let mut lines = generated.ranked_csv.lines();
    assert_eq!(
        format!("{}\n", lines.next().expect("header")),
        RANKED_CSV_HEADER
    );
    let order = lines
        .map(|line| line.split(',').nth(1).expect("case id").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(order, ["00003", "00002", "00001"], "slowest ratio first");
    assert!(
        csv_row(&generated.ranked_csv, "00002").ends_with(",true,3"),
        "1 ms reference must be flagged below_resolution"
    );
    assert!(csv_row(&generated.ranked_csv, "00003").ends_with(",false,3"));
    let summary: serde_json::Value =
        serde_json::from_str(&generated.summary_json).expect("summary json");
    assert_eq!(summary["measurement_boundary"], "cli_case_wall_time");
    assert_eq!(summary["ranked_schema"], "redline-testing-ranked-v2");
}

#[test]
fn report_block_states_ratio_and_measurement_boundary() {
    let generated = generate_report(&three_cases());
    let readme = &generated.readme;
    for expected in [
        "median per-case latency ratio **2.00x** (RedlineDB/SQLite, lower is better)",
        "p95 **10.00x**, worst **10.00x**, faster **1/3**",
        "**1** cases have a SQLite median under 3 ms (`below_resolution`)",
        "**Measurement boundary:** per-case CLI process wall time (`cli_case_wall_time`",
        "lane=`run --suite all --workers auto --repetitions 3 --warmup 0`, workers=128",
        "not a tuned benchmark.",
        "| RedlineDB median ns | Ratio | Gap |",
        "| SLOW | P0 | memory | SMOKE | 4000000 | 40000000 | 10.00x | -900.00% |",
        "| FASTER | P0 | memory | SMOKE | 10000000 | 5000000 | 0.50x | +50.00% |",
    ] {
        assert!(
            readme.contains(expected),
            "missing {expected:?} in:\n{readme}"
        );
    }
    assert!(!readme.contains("median gap"), "{readme}");
    assert!(!readme.contains("Improvement"), "{readme}");
}

#[test]
fn latency_svgs_use_ratio_bands_without_mixed_estimators() {
    let generated = generate_report(&three_cases());
    let [latency, histogram, median] = &generated.svgs[..] else {
        panic!("expected three SVGs");
    };
    for svg in [latency, histogram] {
        for label in [
            "&lt;1x faster",
            "1-2x",
            "2-5x",
            "5-10x",
            "10-50x",
            "&gt;=50x",
        ] {
            assert!(svg.contains(label), "missing band {label}:\n{svg}");
        }
        assert!(!svg.contains('%'), "percent gap in ratio chart:\n{svg}");
    }
    assert!(latency.contains("Median ratio") && latency.contains("2.00x"));
    assert!(latency.contains("1/3"), "{latency}");
    assert!(histogram.contains("SQLite under 3 ms"), "{histogram}");
    assert!(median.contains("Median ratio"), "{median}");
    assert!(
        median.contains("not RedlineDB p50 / SQLite p50"),
        "median chart must say its ratio is not the quotient of its p50s:\n{median}"
    );
    assert!(!median.contains("Median gap"), "{median}");
}

#[test]
fn zero_reference_median_fails_the_report() {
    let err = try_generate("sqlite_parity", &raw_case("00009", "ZERO", 0, 5_000_000))
        .err()
        .expect("a zero reference median must not be ranked");
    let message = format!("{err:#}");
    assert!(message.contains("rank case 00009"), "{message}");
    assert!(message.contains("non-zero SQLite reference"), "{message}");
}

#[test]
fn beyond_sqlite_feature_records_are_not_latency_ranked() {
    // Feature-metadata records carry zero timings by construction.
    let generated = try_generate("beyond_sqlite", &raw_case("BEYOND-001", "FEATURE", 0, 0))
        .expect("beyond report");
    assert_eq!(generated.ranked_csv, RANKED_CSV_HEADER);
}

fn ranked_fixture(ratios: &[u128]) -> Vec<RawRecord> {
    ratios
        .iter()
        .enumerate()
        .map(|(index, ratio)| {
            let line = raw_case(&format!("{index:05}"), "CASE", 1_000_000, ratio * 1_000_000);
            serde_json::from_str(line.lines().next().expect("record")).expect("raw record")
        })
        .collect()
}

#[test]
fn summary_uses_per_case_ratio_order_statistics() {
    let ranked = rank_cases(&ranked_fixture(&(1..=20).collect::<Vec<_>>())).expect("rank");
    let summary = summarize(&ranked);
    assert_eq!(summary.cases, 20);
    assert_eq!(summary.median_ratio, 11.0);
    assert_eq!(summary.p95_ratio, 19.0);
    assert_eq!(summary.worst_ratio, 20.0);
    assert_eq!(summary.faster, 0, "equal or slower cases are never faster");
    assert_eq!(summary.below_resolution, 20);
}
