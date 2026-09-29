//! Case-level latency statistics over a runner's raw JSONL (R4-02 / BM3-06).
//!
//! The unit is the case. Each eligible case contributes one number, the
//! ratio of its medians: the median RedlineDB duration over the median
//! SQLite duration across its measured repetitions (lower is better). The
//! headline is the median and the nearest-rank p95 of those per-case
//! ratios, and a case is faster only when its ratio is below 1. Sample
//! counts are reported beside them under their own names, and the pooled
//! sample-ratio median and exclusive-decile p90 are kept as separately
//! named diagnostics: pooling lets a heavily repeated case outvote the
//! others, so it is never the headline.
//!
//! Validation comes before aggregation. A malformed row or a sample that
//! occurs twice in its case is an error. A case is eligible only when
//! every one of its rows passed, it has measured repetitions `1..=k` with
//! no gap, and every measured duration is usable. In publish mode
//! (`expected_repetitions`), every executed case must have exactly
//! `1..=R`, no row may carry an unusable duration, and at least one case
//! must be eligible.

mod cases;

use std::io::BufRead;

use anyhow::{Result, bail};
use serde::Serialize;

use super::records::{RawRow, parse_rows};

pub(crate) use cases::{CaseOutcome, classify_cases};

pub const SUMMARY_SCHEMA: &str = "perf-evidence-summary-v2";

pub const ESTIMATOR: &str = "per-case ratio of medians (RedlineDB median / SQLite median elapsed ns, lower is better); median and nearest-rank p95 across eligible cases";

/// What a summary requires of its input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SummaryOptions {
    /// Publish mode: every executed case has exactly these measured
    /// repetitions, no duration is unusable, and some case is eligible.
    pub expected_repetitions: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JsonlSummary {
    pub schema_version: &'static str,
    pub estimator: &'static str,
    /// `Some(R)` in publish mode; `None` for a diagnostic summary.
    pub expected_repetitions: Option<usize>,
    /// Every case in the input: eligible + failed + skipped + incomplete.
    pub cases: usize,
    pub eligible_cases: usize,
    /// Cases with any failed row, including a failed warmup and a case
    /// that failed at selection.
    pub failed_cases: usize,
    pub skipped_cases: usize,
    /// Executed cases that did not fail but have no complete, usable set
    /// of measured repetitions: a gap, none at all, or an invalid row.
    pub incomplete_cases: usize,
    /// Passed measured rows whose durations cannot form a ratio.
    pub invalid_rows: usize,
    pub faster_cases: usize,
    pub case_ratio_median: Option<f64>,
    pub case_ratio_p95: Option<f64>,
    /// Measured samples of the eligible cases.
    pub measured_samples: usize,
    /// Of those, samples whose RedlineDB duration beat SQLite's.
    pub faster_samples: usize,
    pub pooled_sample_ratio_median: Option<f64>,
    /// `statistics.quantiles(ratios, n=10)[-1]`; needs ten samples.
    pub pooled_sample_ratio_p90: Option<f64>,
}

impl JsonlSummary {
    pub fn render(&self) -> String {
        let ratio = |value: Option<f64>| value.map_or_else(|| "n/a".to_owned(), fmt3);
        let mut output = String::new();
        let mut line = |label: &str, value: String| {
            output.push_str(&format!("  {:<28}{value}\n", format!("{label}:")));
        };
        line("estimator", ESTIMATOR.to_owned());
        line(
            "repetitions",
            self.expected_repetitions.map_or_else(
                || "not enforced (diagnostic)".to_owned(),
                |repetitions| format!("{repetitions} per case (enforced)"),
            ),
        );
        line("cases", self.cases.to_string());
        line("eligible cases", self.eligible_cases.to_string());
        line("failed cases", self.failed_cases.to_string());
        line("skipped cases", self.skipped_cases.to_string());
        line("incomplete cases", self.incomplete_cases.to_string());
        line("invalid rows", self.invalid_rows.to_string());
        line(
            "faster cases",
            format!("{}/{}", self.faster_cases, self.eligible_cases),
        );
        line("case ratio median", ratio(self.case_ratio_median));
        line("case ratio p95", ratio(self.case_ratio_p95));
        line("measured samples", self.measured_samples.to_string());
        line(
            "faster samples",
            format!("{}/{}", self.faster_samples, self.measured_samples),
        );
        line(
            "pooled sample ratio median",
            ratio(self.pooled_sample_ratio_median),
        );
        line(
            "pooled sample ratio p90",
            ratio(self.pooled_sample_ratio_p90),
        );
        output
    }
}

fn fmt3(value: f64) -> String {
    format!("{value:.3}")
}

/// A diagnostic summary: rows must parse and be unique, but repetitions
/// are not enforced.
pub fn summarize_jsonl(reader: impl BufRead) -> Result<JsonlSummary> {
    summarize_jsonl_with(reader, SummaryOptions::default())
}

pub fn summarize_jsonl_with(reader: impl BufRead, options: SummaryOptions) -> Result<JsonlSummary> {
    summarize_rows(&parse_rows(reader)?, options)
}

pub(crate) fn summarize_rows(rows: &[RawRow], options: SummaryOptions) -> Result<JsonlSummary> {
    let table = classify_cases(rows, options)?;
    let mut summary = JsonlSummary {
        schema_version: SUMMARY_SCHEMA,
        estimator: ESTIMATOR,
        expected_repetitions: options.expected_repetitions,
        cases: table.cases.len(),
        eligible_cases: 0,
        failed_cases: 0,
        skipped_cases: 0,
        incomplete_cases: 0,
        invalid_rows: table.invalid_rows,
        faster_cases: 0,
        case_ratio_median: None,
        case_ratio_p95: None,
        measured_samples: 0,
        faster_samples: 0,
        pooled_sample_ratio_median: None,
        pooled_sample_ratio_p90: None,
    };
    let mut case_ratios = Vec::new();
    let mut sample_ratios = Vec::new();
    for outcome in table.cases.values() {
        let eligible = match outcome {
            CaseOutcome::Failed => {
                summary.failed_cases += 1;
                continue;
            }
            CaseOutcome::Skipped => {
                summary.skipped_cases += 1;
                continue;
            }
            CaseOutcome::Incomplete => {
                summary.incomplete_cases += 1;
                continue;
            }
            CaseOutcome::Eligible(eligible) => eligible,
        };
        let case_ratio = eligible.ratio();
        summary.eligible_cases += 1;
        summary.faster_cases += usize::from(case_ratio < 1.0);
        case_ratios.push(case_ratio);
        for timing in &eligible.timings {
            summary.measured_samples += 1;
            summary.faster_samples += usize::from(timing.target_ns < timing.reference_ns);
            sample_ratios.push(timing.ratio());
        }
    }
    if options.expected_repetitions.is_some() && summary.eligible_cases == 0 {
        bail!(
            "no eligible case among {} (failed {}, skipped {}, incomplete {}): nothing to publish",
            summary.cases,
            summary.failed_cases,
            summary.skipped_cases,
            summary.incomplete_cases
        );
    }
    case_ratios.sort_by(f64::total_cmp);
    sample_ratios.sort_by(f64::total_cmp);
    summary.case_ratio_median = median(&case_ratios);
    summary.case_ratio_p95 = nearest_rank(&case_ratios, 95);
    summary.pooled_sample_ratio_median = median(&sample_ratios);
    summary.pooled_sample_ratio_p90 =
        (sample_ratios.len() >= 10).then(|| exclusive_decile_p90(&sample_ratios));
    Ok(summary)
}

fn median_u64(values: impl Iterator<Item = u64>) -> f64 {
    let mut values = values.map(|value| value as f64).collect::<Vec<_>>();
    values.sort_by(f64::total_cmp);
    median(&values).unwrap_or(f64::NAN)
}

pub(crate) fn median(sorted: &[f64]) -> Option<f64> {
    match sorted.len() {
        0 => None,
        count if count % 2 == 1 => Some(sorted[count / 2]),
        count => Some((sorted[count / 2 - 1] + sorted[count / 2]) / 2.0),
    }
}

/// The smallest value with at least `percent`% of the values at or below it.
pub(crate) fn nearest_rank(sorted: &[f64], percent: usize) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (percent * sorted.len()).div_ceil(100).max(1);
    Some(sorted[rank - 1])
}

/// Match `statistics.quantiles(values, n=10)[-1]` from the retired reference.
fn exclusive_decile_p90(sorted: &[f64]) -> f64 {
    const QUANTILES: usize = 10;
    const INDEX: usize = 9;

    let sample_boundaries = sorted.len() + 1;
    let scaled = INDEX * sample_boundaries;
    let boundary = (scaled / QUANTILES).clamp(1, sorted.len() - 1);
    let remainder = scaled - boundary * QUANTILES;
    (sorted[boundary - 1] * (QUANTILES - remainder) as f64 + sorted[boundary] * remainder as f64)
        / QUANTILES as f64
}

#[cfg(test)]
#[path = "summary_tests.rs"]
mod tests;
