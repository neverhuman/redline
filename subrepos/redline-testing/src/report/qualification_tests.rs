//! SQLite corpus badge and report-block scope tests (L-01/SQ-01).
//!
//! These drive `generate()` end to end and read the README it writes, so they
//! pin the badge and block text readers see, not an internal helper.

use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;

use super::generate;
use super::test_fixtures as fixture;
use super::types::ReportOptions;
use super::utils::sha256_hex;

pub(super) const README: &str = "# Report\n\n<p align=\"center\">\n  <!-- sqlite-parity-badge:begin -->\n  old badge<!-- sqlite-parity-badge:end -->\n</p>\n\n<a id=\"sqlite-parity-status\"></a>\n<!-- sqlite-parity-report:begin -->\n<!-- sqlite-parity-report:end -->\n";

const RUNNER_SHA256: &str = "b28c41d40009bfe7c98832abda695c3b9d2624871c4c886991601718d8e70a78";
const ORACLE_SHA256: &str = "e99d817b62f1ad9ead02b8d4e410fea9d736ee82c35b9a7daa6644d4d3e5ae3a";
const SOURCE_SHA256: &str = "daac7524c76944c99fdaf6ac397034c9aa5634906f035771ba83d7fd10e54ccc";

pub(super) fn record(
    case_id: &str,
    name: &str,
    role: &str,
    repetition: Option<usize>,
    status: &str,
) -> String {
    let timed = status != "skipped";
    let value = serde_json::json!({
        "case_id": case_id,
        "name": name,
        "case_file": format!("{name}.rs"),
        "priority": "P0",
        "profile": "memory",
        "category": "SMOKE",
        "sample_role": role,
        "repetition_index": repetition,
        "status": status,
        "reference_elapsed_ns": if timed { 2_000_000u128 } else { 0 },
        "target_elapsed_ns": if timed { 4_000_000u128 } else { 0 },
        "reference_executable_sha256": if timed { ORACLE_SHA256 } else { "" },
        "target_executable_sha256": if timed { fixture::TARGET_SHA256 } else { "" },
        "reference_version": if timed { fixture::SQLITE_VERSION } else { "" },
    });
    format!("{value}\n")
}

pub(super) fn passed(case_id: &str, name: &str) -> String {
    (1..=3usize)
        .map(|rep| {
            record(
                case_id,
                name,
                &format!("measured:{rep}"),
                Some(rep),
                "passed",
            )
        })
        .collect()
}

/// A case whose three measured samples all failed. (It used to have one
/// sample only, which the per-case sample check now rejects.)
pub(super) fn failed(case_id: &str, name: &str) -> String {
    (1..=3usize)
        .map(|rep| {
            record(
                case_id,
                name,
                &format!("measured:{rep}"),
                Some(rep),
                "failed",
            )
        })
        .collect()
}

pub(super) fn skipped(case_id: &str, name: &str) -> String {
    record(case_id, name, "skipped", None, "skipped")
}

/// Three cases whose ids are declared deviations (fts5, highlight, rtree).
pub(super) fn three_passed() -> String {
    [
        passed("00093", "CREATE_VIRTUAL_TABLE_FTS5_OPTIONAL"),
        passed("00094", "FTS5_HIGHLIGHT_OPTIONAL"),
        passed("00095", "CREATE_VIRTUAL_TABLE_RTREE_OPTIONAL"),
    ]
    .concat()
}

/// Processed official evidence bound to `raw_text`, recording `counts` as
/// (total, passed, failed, skipped) for sqlite_parity and run `status`.
pub(super) fn evidence(raw_text: &str, status: &str, counts: [usize; 4]) -> serde_json::Value {
    let [total, passed, failed, skipped] = counts;
    serde_json::json!({
        "schema_version": "redline-testing-official-evidence-processed-v1",
        "source_sha256": SOURCE_SHA256,
        "status": status,
        "official_evidence": {
            "schema_version": "redline-testing-official-evidence-v1",
            "runner": {
                "binary_path": fixture::RUNNER_PATH,
                "version": "redline-testing 1.0.1",
                "binary_sha256": RUNNER_SHA256,
            },
            "target": {
                "path": fixture::TARGET_PATH,
                "sha256": fixture::TARGET_SHA256,
                "version": "redlinedb v4.1.0 (SQLite 3.45.1 compatibility)",
            },
            "sqlite": {
                "path": fixture::SQLITE_PATH,
                "version": fixture::SQLITE_VERSION,
                "sha256": ORACLE_SHA256,
            },
            "status": status,
            "command_line": ["/ci/redline-testing", "run", "--suite", "all", "--workers", "auto"],
            "workers": "128",
        },
        "suite_summaries": {
            "sqlite_parity": {
                "total": total,
                "passed": passed,
                "failed": failed,
                "skipped": skipped,
                "raw_sha256": sha256_hex(raw_text),
            }
        },
    })
}

pub(super) fn options(root: &Path, evidence: Option<PathBuf>, raw_text: &str) -> ReportOptions {
    ReportOptions {
        suite: "sqlite_parity".to_owned(),
        input: root.join("raw.jsonl"),
        local_diagnostics: evidence.is_none(),
        // These evidence fixtures record no run provenance.
        historical_run: evidence.is_some(),
        run_provenance: None,
        official_evidence: evidence,
        out_dir: root.join("out"),
        readme: root.join("README.md"),
        plot: None,
        performance_histogram_plot: None,
        median_test_performance_plot: Some(root.join("median.svg")),
        readme_latency: true,
        jankurai_score: None,
        updated_date: "2026-09-24".to_owned(),
        expected_repetitions: Some(3),
        expected_warmup: Some(0),
        check: false,
        case_manifest: Some(fixture::case_ids(raw_text)),
    }
}

/// Render `raw_text` into `readme` and return the README afterwards.
pub(super) fn render(raw_text: &str, evidence: Option<serde_json::Value>, readme: &str) -> String {
    let root = tempfile::Builder::new()
        .prefix("redline-testing-qualification-")
        .tempdir()
        .expect("temp root");
    fs::write(root.path().join("raw.jsonl"), raw_text).expect("raw");
    fs::write(root.path().join("README.md"), readme).expect("readme");
    let evidence = evidence.map(|value| {
        let path = root.path().join("official-evidence.processed.json");
        fs::write(&path, serde_json::to_string_pretty(&value).expect("json")).expect("evidence");
        path
    });
    generate(options(root.path(), evidence, raw_text)).expect("report should generate");
    fs::read_to_string(root.path().join("README.md")).expect("rendered README")
}

pub(super) fn badge(readme: &str) -> &str {
    let begin = "<!-- sqlite-parity-badge:begin -->\n";
    let start = readme.find(begin).expect("badge begin") + begin.len();
    let end = readme
        .find("<!-- sqlite-parity-badge:end -->")
        .expect("badge end");
    &readme[start..end]
}

pub(super) fn report_block(readme: &str) -> &str {
    let begin = "<!-- sqlite-parity-report:begin -->\n";
    let start = readme.find(begin).expect("report begin") + begin.len();
    let end = readme
        .find("<!-- sqlite-parity-report:end -->")
        .expect("report end");
    &readme[start..end]
}

#[test]
fn sqlite_badge_complete_is_scoped_green() {
    let raw = three_passed();
    let readme = render(&raw, Some(evidence(&raw, "passed", [3, 3, 0, 0])), README);
    assert_eq!(
        badge(&readme),
        "  <a href=\"#sqlite-parity-status\"><img src=\"https://img.shields.io/badge/SQLite%20SQL%2FCLI%20corpus-3%2F3%20%C2%B7%200%20failed%20%C2%B7%200%20skipped%20%C2%B7%203.53.1-brightgreen\" alt=\"SQLite SQL/CLI corpus: 3/3 cases passed, 0 failed, 0 skipped against the SQLite 3.53.1 shell; 3 declared deviations; not full SQLite compatibility\"></a>"
    );
}

#[test]
fn sqlite_badge_failed_is_red() {
    let raw = [three_passed(), failed("00200", "BROKEN")].concat();
    let readme = render(&raw, Some(evidence(&raw, "failed", [4, 3, 1, 0])), README);
    let badge = badge(&readme);
    assert!(
        badge.contains(
            "-3%2F4%20%C2%B7%201%20failed%20%C2%B7%200%20skipped%20%C2%B7%203.53.1-red\""
        ),
        "{badge}"
    );
    assert!(badge.contains("not full SQLite compatibility"), "{badge}");
}

#[test]
fn sqlite_badge_skipped_not_green() {
    let raw = [three_passed(), skipped("00300", "SKIPPED")].concat();
    let readme = render(&raw, Some(evidence(&raw, "passed", [4, 3, 0, 1])), README);
    let badge = badge(&readme);
    assert!(
        !badge.contains("brightgreen"),
        "a skip must not be green: {badge}"
    );
    assert!(
        badge.contains(
            "-3%2F4%20%C2%B7%200%20failed%20%C2%B7%201%20skipped%20%C2%B7%203.53.1-orange\""
        ),
        "{badge}"
    );
}

#[test]
fn sqlite_badge_missing_evidence_unqualified() {
    let readme = render(&three_passed(), None, README);
    let badge = badge(&readme);
    assert!(
        badge.contains("/badge/SQLite%20SQL%2FCLI%20corpus-unqualified-lightgrey\""),
        "{badge}"
    );
    assert!(
        !badge.contains("3%2F3"),
        "no count without evidence: {badge}"
    );
    assert!(
        badge.contains("alt=\"SQLite SQL/CLI corpus: unqualified (no official evidence); not full SQLite compatibility\""),
        "{badge}"
    );
    let block = report_block(&readme);
    assert!(
        block.contains("**Evidence:** unqualified: no official evidence."),
        "{block}"
    );
}

#[test]
fn sqlite_badge_unqualified_when_evidence_counts_disagree() {
    let raw = [three_passed(), skipped("00300", "SKIPPED")].concat();
    // The evidence claims a clean 4/4 that the raw results do not show.
    let readme = render(&raw, Some(evidence(&raw, "passed", [4, 4, 0, 0])), README);
    let badge = badge(&readme);
    assert!(badge.contains("corpus-unqualified-lightgrey"), "{badge}");
    let block = report_block(&readme);
    assert!(
        block.contains(
            "**Evidence:** unqualified: official evidence records 4 total, 4 passed, 0 failed, 0 skipped, but the raw results have 4 total, 3 passed, 0 failed, 1 skipped."
        ),
        "{block}"
    );
}

#[test]
fn report_block_never_claims_full_compatibility() {
    let raw = three_passed();
    let readme = render(&raw, Some(evidence(&raw, "passed", [3, 3, 0, 0])), README);
    let block = report_block(&readme);
    let lower = block.to_lowercase();
    assert!(!lower.contains("sqlite parity"), "parity claim:\n{block}");
    assert!(
        !lower.contains("full sqlite"),
        "full-compatibility claim:\n{block}"
    );
    assert!(!lower.contains("coverage"), "coverage claim:\n{block}");
    assert!(
        block.starts_with(
            "**SQLite SQL/CLI corpus** (redline-testing `sqlite_parity`, SQLite 3.53.1 shell): **3 / 3** cases passed, **0** failed, **0** skipped. Updated 2026-09-24.\n\n"
        ),
        "first line must name the corpus and oracle:\n{block}"
    );
    for expected in [
        "**Scope** (`sqlite_sql_cli`): each case runs one SQL or dot-command script through the `redlinedb` and SQLite shells and compares their output and exit status. It does not test C ABI semantics, the database file format, or prepared-statement state.",
        "**Evidence:** qualified: official evidence run `daac7524c769` records the same 3 total, 3 passed, 0 failed, 0 skipped. Corpus `sqlite_parity` from redline-testing 1.0.1 (runner SHA-256 `b28c41d40009`), corpus SHA-256 unrecorded; oracle SQLite 3.53.1 (binary SHA-256 `e99d817b62f1`), build stamp unrecorded.",
        "**Declared deviations (3):** these cases pass, but RedlineDB produces the compared output without the SQLite feature behind it.",
        "- `00093` CREATE_VIRTUAL_TABLE_FTS5_OPTIONAL: `USING fts5` creates an ordinary table",
        "- `00094` FTS5_HIGHLIGHT_OPTIONAL: ",
        "- `00095` CREATE_VIRTUAL_TABLE_RTREE_OPTIONAL: ",
        "![SQLite SQL/CLI corpus median ratio](",
    ] {
        assert!(
            block.contains(expected),
            "missing {expected:?} in:\n{block}"
        );
    }
    assert!(
        !block.contains("`00096`"),
        "00096 is not in this run:\n{block}"
    );
}

#[test]
fn report_block_prints_recorded_corpus_and_oracle_build() {
    let raw = passed("00001", "PLAIN");
    let mut value = evidence(&raw, "passed", [1, 1, 0, 0]);
    value["official_evidence"]["corpus_sha256"] = "c0ffee".repeat(10).into();
    value["official_evidence"]["oracle_build_stamp"] =
        "36ca143645cf76997d07b66e9244c636b8ccdec64a1d50558259c4e415e6558b\n-O2".into();
    let readme = render(&raw, Some(value), README);
    let block = report_block(&readme);
    assert!(block.contains("corpus SHA-256 `c0ffeec0ffee`"), "{block}");
    assert!(block.contains("build stamp `36ca143645cf`"), "{block}");
    assert!(
        block.contains("**Declared deviations:** none among the cases that passed in this run."),
        "{block}"
    );
}

#[test]
fn retired_metric_blocks_are_removed_not_appended() {
    let raw = three_passed();
    let with_retired = format!(
        "{README}\n## Engine Metrics\n\n<!-- sqlite-parity-metrics:begin -->\n![placeholder](assets/sqlite-parity-ksloc.svg)\n<!-- sqlite-parity-metrics:end -->\n\n## Jankurai Breakdown\n\n<!-- sqlite-jankurai-breakdown:begin -->\n{{\"sqlite_score\": 22}}\n<!-- sqlite-jankurai-breakdown:end -->\n\n## Architecture\n"
    );
    for readme in [README.to_owned(), with_retired.clone()] {
        let rendered = render(&raw, Some(evidence(&raw, "passed", [3, 3, 0, 0])), &readme);
        for marker in [
            "sqlite-parity-metrics:",
            "sqlite-jankurai-breakdown:",
            "sqlite-parity-ksloc.svg",
            "sqlite_score",
        ] {
            assert!(!rendered.contains(marker), "{marker} survived:\n{rendered}");
        }
        // Only the retired blocks go: the badge, the report block and the
        // text around and after them stay, byte for byte.
        assert!(
            rendered.starts_with(
                "# Report\n\n<p align=\"center\">\n  <!-- sqlite-parity-badge:begin -->\n  <a href="
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains("<!-- sqlite-parity-badge:end -->\n</p>\n\n<a id=\"sqlite-parity-status\"></a>\n<!-- sqlite-parity-report:begin -->\n**SQLite SQL/CLI corpus**"),
            "{rendered}"
        );
        let tail = if readme == with_retired {
            "<!-- sqlite-parity-report:end -->\n\n## Engine Metrics\n\n\n## Jankurai Breakdown\n\n\n## Architecture\n"
        } else {
            "<!-- sqlite-parity-report:end -->\n"
        };
        assert!(
            rendered.ends_with(tail),
            "tail is not {tail:?}:\n{rendered}"
        );
    }
}

#[test]
fn report_cli_rejects_placeholder_card_flags() {
    let base = [
        "redline-testing",
        "report",
        "--suite",
        "sqlite_parity",
        "--input",
        "raw.jsonl",
        "--out-dir",
        "out",
        "--readme",
        "README.md",
        "--updated-date",
        "2026-09-24",
    ];
    assert!(crate::cli::Cli::try_parse_from(base).is_ok());
    for flag in [
        "--ksloc-plot",
        "--jankurai-score-plot",
        "--code-shape-plot",
        "--jankurai-comparison-plot",
        "--jankurai-comparison",
    ] {
        let args = base.iter().copied().chain([flag, "card.svg"]);
        assert!(
            crate::cli::Cli::try_parse_from(args).is_err(),
            "{flag} must no longer be accepted"
        );
    }
}

#[test]
fn report_block_lists_shared_rejections_and_oracle_build_deviations_apart() {
    let raw = [
        passed("00093", "CREATE_VIRTUAL_TABLE_FTS5_OPTIONAL"),
        passed("10546", "MEDIAN_REQUIRES_CAPABILITY"),
        passed("11437", "STRING_SOUNDEX_ROBERT"),
    ]
    .concat();
    let readme = render(&raw, Some(evidence(&raw, "passed", [3, 3, 0, 0])), README);
    let block = report_block(&readme);
    for expected in [
        "**Declared deviations (1):** these cases pass, but RedlineDB produces the compared output without the SQLite feature behind it.\n\n- `00093` CREATE_VIRTUAL_TABLE_FTS5_OPTIONAL: ",
        "**Declared shared rejections (1):** the pinned SQLite build lacks the feature, so these cases declare its error; a pass means RedlineDB rejected the statement too, not that the feature works.\n\n- `11437` STRING_SOUNDEX_ROBERT: ",
        "**Declared oracle-build deviations (1):** these cases were written for a different SQLite build; against the pinned SQLite build they check what the reason states.\n\n- `10546` MEDIAN_REQUIRES_CAPABILITY: ",
    ] {
        assert!(
            block.contains(expected),
            "missing {expected:?} in:\n{block}"
        );
    }
    let badge = badge(&readme);
    assert!(badge.contains("; 3 declared deviations;"), "{badge}");
}
