//! A run's rows grouped by case, and each case's outcome under the summary's
//! rules: eligible with a ratio of medians, failed, skipped or incomplete.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};

use super::{SummaryOptions, median_u64};
use crate::perf_evidence::records::{RawRow, RowStatus, SampleKey, Timing};

#[derive(Default)]
struct CaseRows<'a> {
    first_line: BTreeMap<SampleKey, usize>,
    failed: bool,
    skipped: bool,
    measured: BTreeMap<usize, &'a RawRow>,
    invalid_rows: usize,
}

/// What one case of a run amounts to under this module's rules.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CaseOutcome {
    /// Every row passed, with measured repetitions `1..=k` and usable
    /// durations: the case contributes a ratio.
    Eligible(EligibleCase),
    /// Some row failed, a failed warmup and a failed selection included.
    Failed,
    Skipped,
    /// Executed and not failed, but without a complete, usable set of
    /// measured repetitions.
    Incomplete,
}

/// The measured samples of an eligible case.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EligibleCase {
    pub(crate) timings: Vec<Timing>,
}

impl EligibleCase {
    /// The case's ratio of medians: the median RedlineDB duration over the
    /// median SQLite duration across its measured repetitions.
    pub(crate) fn ratio(&self) -> f64 {
        let reference = median_u64(self.timings.iter().map(|timing| timing.reference_ns));
        let target = median_u64(self.timings.iter().map(|timing| timing.target_ns));
        target / reference
    }
}

/// Every case of a run with its outcome, and the passed measured rows
/// whose durations cannot form a ratio.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CaseTable {
    pub(crate) cases: BTreeMap<String, CaseOutcome>,
    pub(crate) invalid_rows: usize,
}

/// Groups `rows` by case and decides each case's outcome. A sample that
/// occurs twice in its case is an error; in publish mode so are an
/// unusable duration and an executed case without exactly `1..=R`.
pub(crate) fn classify_cases(rows: &[RawRow], options: SummaryOptions) -> Result<CaseTable> {
    let mut cases = BTreeMap::<&str, CaseRows<'_>>::new();
    for row in rows {
        let case = cases.entry(row.case_id.as_str()).or_default();
        if let Some(first) = case.first_line.insert(row.sample, row.line) {
            bail!(
                "JSONL line {}: case {}: duplicate sample {} (first on line {first})",
                row.line,
                row.case_id,
                row.sample
            );
        }
        case.failed |= row.status == RowStatus::Failed;
        case.skipped |= row.status == RowStatus::Skipped;
        if let SampleKey::Measured(repetition) = row.sample {
            case.measured.insert(repetition, row);
            if row.status == RowStatus::Passed
                && let Some(defect) = row.timing.as_ref().and_then(Timing::defect)
            {
                if options.expected_repetitions.is_some() {
                    bail!(
                        "JSONL line {}: case {}: {}: {defect}",
                        row.line,
                        row.case_id,
                        row.sample
                    );
                }
                case.invalid_rows += 1;
            }
        }
    }

    let mut table = CaseTable {
        cases: BTreeMap::new(),
        invalid_rows: 0,
    };
    for (case_id, case) in &cases {
        table.invalid_rows += case.invalid_rows;
        let not_run = case.first_line.contains_key(&SampleKey::NotRun);
        if not_run && case.first_line.len() != 1 {
            bail!(
                "case {case_id}: a case that did not run has one not-run row, found {}",
                case.first_line.len()
            );
        }
        let repetitions = case.measured.keys().copied().collect::<BTreeSet<_>>();
        if let Some(expected) = options.expected_repetitions
            && !not_run
            && repetitions != (1..=expected).collect()
        {
            bail!(
                "case {case_id}: expected measured repetitions 1..={expected}, found {:?}",
                repetitions.iter().collect::<Vec<_>>()
            );
        }
        let contiguous =
            !repetitions.is_empty() && repetitions == (1..=repetitions.len()).collect();
        let outcome = if case.failed {
            CaseOutcome::Failed
        } else if case.skipped {
            CaseOutcome::Skipped
        } else if !contiguous || case.invalid_rows > 0 {
            CaseOutcome::Incomplete
        } else {
            CaseOutcome::Eligible(EligibleCase {
                timings: case
                    .measured
                    .values()
                    .filter_map(|row| row.timing)
                    .collect(),
            })
        };
        table.cases.insert((*case_id).to_owned(), outcome);
    }
    Ok(table)
}
