//! Summary statistics and chart bands over per-case latency ratios.
//!
//! Every figure here is a statistic of per-case RedlineDB/SQLite ratios.
//! Absolute medians are reported separately and never mixed into a ratio.

use super::types::{RankedCase, SvgBar};

/// Headline numbers for the latency line, the SVG metrics and the README.
/// With no ranked case there is no ratio at all (`None`), never a 0.00x
/// that would read as a win on a lower-is-better scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LatencySummary {
    pub(crate) cases: usize,
    /// Median of per-case ratios (upper median for an even count).
    pub(crate) median_ratio: Option<f64>,
    /// Nearest-rank 95th percentile of per-case ratios.
    pub(crate) p95_ratio: Option<f64>,
    /// Largest per-case ratio.
    pub(crate) worst_ratio: Option<f64>,
    /// Cases where RedlineDB's median is strictly below SQLite's.
    pub(crate) faster: usize,
    /// Cases whose SQLite median is below the resolution threshold.
    pub(crate) below_resolution: usize,
}

pub(crate) fn summarize(ranked: &[RankedCase]) -> LatencySummary {
    let mut ratios = ranked
        .iter()
        .map(|case| case.latency_ratio)
        .collect::<Vec<_>>();
    ratios.sort_by(|left, right| left.total_cmp(right));
    let at = |index: usize| ratios.get(index).copied();
    let cases = ratios.len();
    let p95_rank = (cases * 95).div_ceil(100).max(1);
    LatencySummary {
        cases,
        median_ratio: at(cases / 2),
        p95_ratio: at(p95_rank - 1),
        worst_ratio: ratios.last().copied(),
        faster: ranked.iter().filter(|case| case.faster).count(),
        below_resolution: ranked.iter().filter(|case| case.below_resolution).count(),
    }
}

pub(crate) fn format_ratio(ratio: f64) -> String {
    format!("{ratio:.2}x")
}

/// A summary ratio, or `n/a` when no case was ranked.
pub(crate) fn format_summary_ratio(ratio: Option<f64>) -> String {
    ratio.map_or_else(|| NO_RATIO.to_owned(), format_ratio)
}

/// What a chart or line shows in place of a ratio or duration no case gave.
pub(crate) const NO_RATIO: &str = "n/a";

/// Ratio bands (RedlineDB/SQLite). The first band holds exactly the cases
/// counted as faster, so the chart and the headline count agree.
pub(crate) fn ratio_histogram_bars(ranked: &[RankedCase]) -> Vec<SvgBar> {
    const BANDS: [(&str, f64); 5] = [
        ("1-2x", 2.0),
        ("2-5x", 5.0),
        ("5-10x", 10.0),
        ("10-50x", 50.0),
        (">=50x", f64::INFINITY),
    ];
    let mut counts = [0usize; BANDS.len() + 1];
    for case in ranked {
        let index = if case.faster {
            0
        } else {
            1 + BANDS
                .iter()
                .position(|(_, upper)| case.latency_ratio < *upper)
                .unwrap_or(BANDS.len() - 1)
        };
        counts[index] = counts[index].saturating_add(1);
    }
    std::iter::once("<1x faster")
        .chain(BANDS.iter().map(|(label, _)| *label))
        .zip(counts)
        .map(|(label, count)| SvgBar {
            label: label.to_owned(),
            value: count as f64,
            value_label: count.to_string(),
        })
        .collect()
}
