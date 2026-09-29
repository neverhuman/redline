//! The plan checks on a run's rows, without the file and its marker: every
//! sample belongs to the plan, every executed case has exactly its warmups
//! and measured repetitions, and the cases are exactly the expected ones.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};

use super::{RunPlan, RunValidation};
use crate::perf_evidence::records::{RawRow, RowStatus, SampleKey};

/// How many case ids an error lists before it abbreviates.
const LISTED_IDS: usize = 10;

#[derive(Default)]
struct CaseSamples {
    first_line: BTreeMap<SampleKey, usize>,
    warmups: usize,
    measured: BTreeSet<usize>,
    not_run: Option<RowStatus>,
    failed: bool,
}

/// The plan checks, without the file and its marker.
pub(crate) fn validate_rows(rows: &[RawRow], plan: &RunPlan) -> Result<RunValidation> {
    if plan.repetitions == 0 {
        bail!("a run plan needs at least one measured repetition");
    }
    if let Some(manifest) = &plan.case_manifest
        && manifest.len() != plan.expected_cases
    {
        bail!(
            "the case manifest lists {} cases, but {} are expected",
            manifest.len(),
            plan.expected_cases
        );
    }
    let mut cases = BTreeMap::<&str, CaseSamples>::new();
    for row in rows {
        let label = format!("JSONL line {}: case {}", row.line, row.case_id);
        check_sample(row, plan).with_context(|| label.clone())?;
        let case = cases.entry(row.case_id.as_str()).or_default();
        if let Some(first) = case.first_line.insert(row.sample, row.line) {
            bail!(
                "{label}: duplicate sample {} (first on line {first})",
                row.sample
            );
        }
        match row.sample {
            SampleKey::Warmup(_) => case.warmups += 1,
            SampleKey::Measured(repetition) => {
                case.measured.insert(repetition);
            }
            SampleKey::NotRun => case.not_run = Some(row.status),
        }
        case.failed |= row.status == RowStatus::Failed;
    }

    let mut validation = RunValidation {
        records: rows.len(),
        cases: cases.len(),
        executed_cases: 0,
        skipped_cases: 0,
        not_run_cases: 0,
        failed_cases: 0,
        raw_sha256: String::new(),
    };
    let expected_measured = (1..=plan.repetitions).collect::<BTreeSet<_>>();
    for (case_id, case) in &cases {
        match case.not_run {
            Some(status) => {
                if case.first_line.len() != 1 {
                    bail!(
                        "case {case_id}: a case that did not run has one not-run row, found {} rows",
                        case.first_line.len()
                    );
                }
                if status == RowStatus::Skipped {
                    validation.skipped_cases += 1;
                } else {
                    validation.not_run_cases += 1;
                }
            }
            None => {
                if case.warmups != plan.warmup {
                    bail!(
                        "case {case_id}: expected {} warmup samples, found {}",
                        plan.warmup,
                        case.warmups
                    );
                }
                if case.measured != expected_measured {
                    bail!(
                        "case {case_id}: expected measured repetitions 1..={}, found {:?}",
                        plan.repetitions,
                        case.measured.iter().collect::<Vec<_>>()
                    );
                }
                validation.executed_cases += 1;
            }
        }
        validation.failed_cases += usize::from(case.failed);
    }

    let observed = cases
        .keys()
        .map(|id| (*id).to_owned())
        .collect::<BTreeSet<_>>();
    if let Some(manifest) = &plan.case_manifest {
        let missing = manifest.difference(&observed).collect::<Vec<_>>();
        let extra = observed.difference(manifest).collect::<Vec<_>>();
        if !missing.is_empty() || !extra.is_empty() {
            bail!(
                "the run's {} cases are not the manifest's {}: missing {}; not in the manifest {}",
                observed.len(),
                manifest.len(),
                listed(&missing),
                listed(&extra)
            );
        }
    }
    if observed.len() != plan.expected_cases {
        bail!(
            "the run holds {} cases, {} expected",
            observed.len(),
            plan.expected_cases
        );
    }
    Ok(validation)
}

/// A row's sample must belong to the plan.
fn check_sample(row: &RawRow, plan: &RunPlan) -> Result<()> {
    match row.sample {
        SampleKey::Warmup(index) => {
            if plan.warmup == 0 {
                bail!("a warmup sample in a run without warmups");
            }
            if let Some(index) = index
                && index >= plan.warmup
            {
                bail!(
                    "warmup sample_index {index} is not below the {} warmups",
                    plan.warmup
                );
            }
        }
        SampleKey::Measured(repetition) => {
            if repetition > plan.repetitions {
                bail!(
                    "measured:{repetition} is outside repetitions 1..={}",
                    plan.repetitions
                );
            }
        }
        SampleKey::NotRun => {}
    }
    Ok(())
}

fn listed(ids: &[&String]) -> String {
    let shown = ids.iter().take(LISTED_IDS).collect::<Vec<_>>();
    if ids.len() > LISTED_IDS {
        format!("{shown:?} and {} more", ids.len() - LISTED_IDS)
    } else {
        format!("{shown:?}")
    }
}
