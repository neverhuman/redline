//! One disjoint verdict per case, from complete and unique samples (SQ-04).
//!
//! A case is `failed` when any of its records failed, `skipped` when its
//! one record is a policy skip, and `passed` only when every sample passed:
//! a failed warmup is a failed case. Every executed case must have exactly
//! the run's warmups and measured repetitions `1..=R`, each once, and the
//! cases must be exactly the manifest's, so `passed + failed + skipped`
//! is the manifest's size by construction and a missing case cannot hide
//! behind an extra one.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use anyhow::{Result, bail};

use super::types::RawRecord;

/// How many case ids an error lists before it abbreviates.
const LISTED_IDS: usize = 10;

/// The case ids of each verdict; disjoint, and together the manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Verdicts {
    pub(crate) passed: BTreeSet<String>,
    pub(crate) failed: BTreeSet<String>,
    pub(crate) skipped: BTreeSet<String>,
}

impl Verdicts {
    pub(crate) fn total(&self) -> usize {
        self.passed.len() + self.failed.len() + self.skipped.len()
    }
}

/// A sample's identity within its case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Sample {
    /// A warmup, by its `sample_index` when the record carries one.
    Warmup(Option<usize>),
    Measured(usize),
    /// The one record of a case that did not run: a policy skip
    /// (`skipped`) or a selection failure (`not_run`).
    Placeholder,
}

impl fmt::Display for Sample {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Warmup(Some(index)) => write!(f, "warmup (sample {index})"),
            Self::Warmup(None) => f.write_str("warmup"),
            Self::Measured(repetition) => write!(f, "measured:{repetition}"),
            Self::Placeholder => f.write_str("placeholder"),
        }
    }
}

#[derive(Default)]
struct CaseSamples {
    samples: BTreeSet<Sample>,
    warmups: usize,
    measured: BTreeSet<usize>,
    placeholder_status: Option<String>,
    failed: bool,
}

/// Reduces a SQLite-shell suite's raw records to one verdict per case,
/// requiring `warmup` warmups and repetitions `1..=repetitions` of every
/// executed case, each exactly once, and exactly the `manifest_ids` cases.
pub(crate) fn reduce_sqlite_verdicts(
    records: &[RawRecord],
    manifest_ids: &BTreeSet<String>,
    warmup: usize,
    repetitions: usize,
) -> Result<Verdicts> {
    let mut cases = BTreeMap::<&str, CaseSamples>::new();
    for record in records {
        let case_id = record.case_id.as_str();
        let sample = sample_of(record, warmup, repetitions)
            .map_err(|error| anyhow::anyhow!("case {case_id}: {error}"))?;
        let case = cases.entry(case_id).or_default();
        if !case.samples.insert(sample) {
            bail!("case {case_id}: duplicate sample {sample}");
        }
        match sample {
            Sample::Warmup(_) => case.warmups += 1,
            Sample::Measured(repetition) => {
                case.measured.insert(repetition);
            }
            Sample::Placeholder => case.placeholder_status = Some(record.status.clone()),
        }
        case.failed |= record.status == "failed";
    }
    let mut verdicts = Verdicts::default();
    let expected_measured = (1..=repetitions).collect::<BTreeSet<_>>();
    for (case_id, case) in cases.iter() {
        let verdict = match &case.placeholder_status {
            Some(status) => {
                if case.samples.len() != 1 {
                    bail!(
                        "case {case_id}: a case that did not run has one placeholder record, found {} records",
                        case.samples.len()
                    );
                }
                if status == "skipped" {
                    &mut verdicts.skipped
                } else {
                    &mut verdicts.failed
                }
            }
            None => {
                if case.warmups != warmup {
                    bail!(
                        "case {case_id}: expected {warmup} warmup samples but found {}",
                        case.warmups
                    );
                }
                if case.measured != expected_measured {
                    bail!(
                        "case {case_id}: expected measured repetitions 1..={repetitions}, found {:?}",
                        case.measured.iter().collect::<Vec<_>>()
                    );
                }
                if case.failed {
                    &mut verdicts.failed
                } else {
                    &mut verdicts.passed
                }
            }
        };
        verdict.insert((*case_id).to_owned());
    }
    let observed = cases
        .keys()
        .map(|id| (*id).to_owned())
        .collect::<BTreeSet<_>>();
    let missing = manifest_ids.difference(&observed).collect::<Vec<_>>();
    let extra = observed.difference(manifest_ids).collect::<Vec<_>>();
    if !missing.is_empty() || !extra.is_empty() {
        bail!(
            "the raw records' {} cases are not the manifest's {}: missing {}; not in the manifest {}",
            observed.len(),
            manifest_ids.len(),
            listed(&missing),
            listed(&extra)
        );
    }
    debug_assert_eq!(verdicts.total(), manifest_ids.len());
    Ok(verdicts)
}

/// The sample a record claims to be, checked against its fields and the
/// run's sample plan.
fn sample_of(record: &RawRecord, warmup: usize, repetitions: usize) -> Result<Sample> {
    let role = record.sample_role.as_str();
    let status = record.status.as_str();
    let sample = if role == "warmup" {
        if warmup == 0 {
            bail!("a warmup sample in a run without warmups");
        }
        if let Some(index) = record.sample_index
            && index >= warmup
        {
            bail!("warmup sample_index {index} is not below the {warmup} warmups");
        }
        Sample::Warmup(record.sample_index)
    } else if let Some(repetition) = role.strip_prefix("measured:") {
        let Ok(repetition) = repetition.parse::<usize>() else {
            bail!("sample_role {role:?} names no repetition");
        };
        if record.repetition_index != Some(repetition) {
            bail!(
                "{role} carries repetition_index {:?}",
                record.repetition_index
            );
        }
        if !(1..=repetitions).contains(&repetition) {
            bail!("{role} is outside repetitions 1..={repetitions}");
        }
        if let Some(index) = record.sample_index
            && index != warmup + repetition - 1
        {
            bail!(
                "{role} carries sample_index {index}, expected {}",
                warmup + repetition - 1
            );
        }
        Sample::Measured(repetition)
    } else if (role == "skipped" && status == "skipped")
        || (role == "not_run" && status == "failed")
    {
        Sample::Placeholder
    } else {
        bail!("record with sample_role {role:?} and status {status:?} is no known sample");
    };
    if !matches!(sample, Sample::Placeholder) {
        if record.repetition_index.is_some() && matches!(sample, Sample::Warmup(_)) {
            bail!(
                "warmup carries repetition_index {:?}",
                record.repetition_index
            );
        }
        if !matches!(status, "passed" | "failed") {
            bail!("{sample} has status {status:?}");
        }
    }
    Ok(sample)
}

/// Up to `LISTED_IDS` ids, then how many more.
fn listed(ids: &[&String]) -> String {
    let shown = ids.iter().take(LISTED_IDS).collect::<Vec<_>>();
    if ids.len() > LISTED_IDS {
        format!("{shown:?} and {} more", ids.len() - LISTED_IDS)
    } else {
        format!("{shown:?}")
    }
}

/// One verdict per case with no sample plan or manifest, for suites whose
/// records are not samples (beyond_sqlite): failed over skipped over passed.
pub(crate) fn case_verdicts(records: &[RawRecord]) -> Verdicts {
    let mut statuses = BTreeMap::<&str, BTreeSet<&str>>::new();
    for record in records {
        statuses
            .entry(record.case_id.as_str())
            .or_default()
            .insert(record.status.as_str());
    }
    let mut verdicts = Verdicts::default();
    for (case_id, statuses) in statuses {
        let verdict = if statuses.contains("failed") {
            &mut verdicts.failed
        } else if statuses.contains("skipped") {
            &mut verdicts.skipped
        } else {
            &mut verdicts.passed
        };
        verdict.insert(case_id.to_owned());
    }
    verdicts
}

#[cfg(test)]
#[path = "verdicts_tests.rs"]
mod tests;
