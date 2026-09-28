//! The statistics of a release bench bundle: per-run cohort ratios, their
//! spread across a label's K runs, and a change against the previous label
//! that is reported only when it clears that spread (R4-07).

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use serde::Serialize;

use super::summary::{CaseOutcome, median, nearest_rank};

/// A per-label statistic: the median of its K per-run values, their range,
/// and each run's value in run order.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Spread {
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub per_run: Vec<f64>,
}

impl Spread {
    fn of(per_run: Vec<f64>) -> Self {
        let mut sorted = per_run.clone();
        sorted.sort_by(f64::total_cmp);
        Self {
            value: median(&sorted).unwrap_or(f64::NAN),
            min: sorted.first().copied().unwrap_or(f64::NAN),
            max: sorted.last().copied().unwrap_or(f64::NAN),
            per_run,
        }
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.min <= other.max && other.min <= self.max
    }
}

/// Whether a label's change against the previous label clears the noise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseVerdict {
    /// The two labels' min-max ranges across their runs overlap.
    WithinNoise,
    /// The ranges do not overlap.
    ExceedsNoise,
    /// A label has fewer than two runs, so no spread was measured.
    Unassessed,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Delta {
    pub previous: String,
    /// (this median / previous median - 1) * 100; negative is faster.
    pub median_change_pct: f64,
    pub verdict: NoiseVerdict,
}

/// A label's ratios on one cohort of cases.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CohortStats {
    pub cases: usize,
    /// Median across the cohort's cases of each case's ratio of medians.
    pub median: Spread,
    /// Nearest-rank p95 across the cohort's cases.
    pub p95: Spread,
    pub delta_vs_previous: Option<Delta>,
}

/// The ratios of `cohort` in each of a label's runs, or `None` for an
/// empty cohort. Every case of the cohort must be eligible in every run.
pub(crate) fn cohort_stats(
    runs: &[&BTreeMap<String, CaseOutcome>],
    cohort: &BTreeSet<String>,
) -> Result<Option<CohortStats>> {
    if cohort.is_empty() || runs.is_empty() {
        return Ok(None);
    }
    let mut medians = Vec::with_capacity(runs.len());
    let mut p95s = Vec::with_capacity(runs.len());
    for run in runs {
        let mut ratios = Vec::with_capacity(cohort.len());
        for case_id in cohort {
            match run.get(case_id) {
                Some(CaseOutcome::Eligible(case)) => ratios.push(case.ratio()),
                _ => bail!("case {case_id} is in the cohort but not eligible in every run"),
            }
        }
        ratios.sort_by(f64::total_cmp);
        medians.extend(median(&ratios));
        p95s.extend(nearest_rank(&ratios, 95));
    }
    Ok(Some(CohortStats {
        cases: cohort.len(),
        median: Spread::of(medians),
        p95: Spread::of(p95s),
        delta_vs_previous: None,
    }))
}

/// The change of `current`'s median against `previous`'s, judged against
/// both labels' run-to-run spread.
pub(crate) fn delta(previous_label: &str, previous: &CohortStats, current: &CohortStats) -> Delta {
    let verdict = if previous.median.per_run.len() < 2 || current.median.per_run.len() < 2 {
        NoiseVerdict::Unassessed
    } else if previous.median.overlaps(&current.median) {
        NoiseVerdict::WithinNoise
    } else {
        NoiseVerdict::ExceedsNoise
    };
    Delta {
        previous: previous_label.to_owned(),
        median_change_pct: (current.median.value / previous.median.value - 1.0) * 100.0,
        verdict,
    }
}
