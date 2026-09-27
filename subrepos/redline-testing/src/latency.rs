//! Per-case latency comparison shared by the runner-side evidence writer and
//! the `report` renderer, so both publish the same numbers.
//!
//! Each case is summarised as the ratio of the RedlineDB median to the SQLite
//! median (lower is better). There is no reference floor: a case where
//! RedlineDB is slower is never reported as faster. References below
//! [`RESOLUTION_NS`] are flagged instead of being rewritten.

use anyhow::{Result, bail};

/// Stable identifier of what one latency sample measures.
pub(crate) const MEASUREMENT_BOUNDARY: &str = "cli_case_wall_time";

/// Human wording of [`MEASUREMENT_BOUNDARY`] for READMEs and charts.
pub(crate) const MEASUREMENT_BOUNDARY_TEXT: &str = "per-case CLI process wall time";

/// SQLite medians under this are near process-spawn resolution; such cases
/// keep their raw ratio and are only flagged `below_resolution`.
pub(crate) const RESOLUTION_NS: u128 = 3_000_000;

/// Identifies the ranked.csv column layout. Files without `latency_ratio`
/// and `gap_pct_raw` columns predate it and used the floored formula.
pub(crate) const RANKED_CSV_SCHEMA: &str = "redline-testing-ranked-v2";

/// Column header of ranked.csv for [`RANKED_CSV_SCHEMA`].
pub(crate) const RANKED_CSV_HEADER: &str = "rank,case_id,name,case_file,priority,profile,category,sqlite_median_ns,redline_median_ns,latency_ratio,gap_pct_raw,below_resolution,samples\n";

/// RedlineDB time divided by SQLite time for the same case.
///
/// A zero on either side means the timing is missing, so it is an error
/// rather than an infinitely fast or infinitely slow case.
pub(crate) fn latency_ratio(reference_ns: u128, target_ns: u128) -> Result<f64> {
    if reference_ns == 0 {
        bail!("latency ratio needs a non-zero SQLite reference time (target {target_ns} ns)");
    }
    if target_ns == 0 {
        bail!("latency ratio needs a non-zero RedlineDB time (reference {reference_ns} ns)");
    }
    let ratio = target_ns as f64 / reference_ns as f64;
    if !ratio.is_finite() || ratio <= 0.0 {
        bail!("latency ratio {reference_ns} ns / {target_ns} ns is not a positive finite number");
    }
    Ok(ratio)
}

/// Per-case latency comparison derived from the two medians.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CaseLatency {
    /// RedlineDB/SQLite; below 1.0 means RedlineDB was faster.
    pub(crate) latency_ratio: f64,
    /// `(sqlite - redline) / sqlite * 100` with no floor; negative is slower.
    pub(crate) gap_pct: f64,
    /// The SQLite median is under [`RESOLUTION_NS`].
    pub(crate) below_resolution: bool,
    /// RedlineDB's median is strictly lower than SQLite's.
    pub(crate) faster: bool,
}

impl CaseLatency {
    pub(crate) fn from_medians(reference_ns: u128, target_ns: u128) -> Result<Self> {
        let latency_ratio = latency_ratio(reference_ns, target_ns)?;
        Ok(Self {
            latency_ratio,
            gap_pct: (1.0 - latency_ratio) * 100.0,
            below_resolution: reference_ns < RESOLUTION_NS,
            faster: target_ns < reference_ns,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{CaseLatency, RESOLUTION_NS, latency_ratio};

    #[test]
    fn improvement_is_raw_ratio_without_floor() {
        let case = CaseLatency::from_medians(1_000_000, 2_000_000).unwrap();
        assert_eq!(case.latency_ratio, 2.0);
        assert_eq!(case.gap_pct, -100.0);
        assert!(!case.faster);
        assert!(case.below_resolution);
    }

    #[test]
    fn trigger_row_ratio() {
        let case = CaseLatency::from_medians(2_916_608, 612_825_241).unwrap();
        assert!((case.latency_ratio - 210.1157375).abs() < 1e-6);
        assert!((case.gap_pct - -20_911.573_752_797_77).abs() < 1e-6);
        assert!(!case.faster);
        assert!(case.below_resolution);
    }

    #[test]
    fn ratio_scale_invariant() {
        let base = latency_ratio(1_234_567, 9_876_543).unwrap();
        for scale in [1u128, 10, 1_000, 1_000_000] {
            let scaled = latency_ratio(1_234_567 * scale, 9_876_543 * scale).unwrap();
            assert!((scaled - base).abs() < 1e-12, "scale {scale}");
        }
        // Scaling changes the resolution flag, never the ratio or the sign.
        let small = CaseLatency::from_medians(1_000, 1_500).unwrap();
        let large = CaseLatency::from_medians(1_000_000_000, 1_500_000_000).unwrap();
        assert_eq!(small.latency_ratio, large.latency_ratio);
        assert_eq!(small.gap_pct, large.gap_pct);
        assert!(small.below_resolution && !large.below_resolution);
    }

    #[test]
    fn no_faster_case_has_target_ge_reference() {
        let timings = [
            1u128,
            999_999,
            1_000_000,
            2_999_999,
            RESOLUTION_NS,
            3_000_001,
            10_000_000,
            612_825_241,
            u64::MAX as u128,
        ];
        for reference in timings {
            for target in timings {
                let case = CaseLatency::from_medians(reference, target).unwrap();
                if target >= reference {
                    assert!(!case.faster, "{reference} vs {target} marked faster");
                    assert!(case.latency_ratio >= 1.0, "{reference} vs {target}");
                    assert!(case.gap_pct <= 0.0, "{reference} vs {target}");
                } else {
                    assert!(case.faster, "{reference} vs {target} not marked faster");
                }
            }
        }
    }

    #[test]
    fn zero_reference_rejected() {
        let err = latency_ratio(0, 5_000_000).unwrap_err();
        assert!(
            err.to_string().contains("non-zero SQLite reference"),
            "{err}"
        );
        assert!(CaseLatency::from_medians(0, 1).is_err());
    }

    #[test]
    fn zero_target_rejected() {
        let err = latency_ratio(5_000_000, 0).unwrap_err();
        assert!(err.to_string().contains("non-zero RedlineDB"), "{err}");
    }
}
