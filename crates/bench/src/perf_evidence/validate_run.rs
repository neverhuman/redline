//! `validate-run`: is a runner's raw file the complete experiment that was
//! asked for? (BM3-01)
//!
//! A perf lane may summarize a run only after this check passes. Every row
//! must parse (`records::parse_rows`) and be one sample of the run's plan:
//! each executed case has exactly `warmup` warmups and measured
//! repetitions `1..=repetitions`, each once; a case that did not run is one
//! placeholder row. The cases must be exactly `expected_cases` in number
//! and, given the runner's own corpus listing, exactly its ids. The
//! runner's completion marker (`<raw>.complete.json`) must exist and name
//! this file's SHA-256, record count and case count: the runner writes it
//! only when a suite finished, so an interrupted run never validates.
//!
//! The check is about completeness, not correctness: failed cases are
//! counted and left to the lane's known-failures filter.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::records::{RawRow, RowStatus, SampleKey, parse_rows};

pub const COMPLETION_SCHEMA: &str = "redline-testing-raw-complete-v1";

/// How many case ids an error lists before it abbreviates.
const LISTED_IDS: usize = 10;

/// The experiment a run was asked to perform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunPlan {
    pub expected_cases: usize,
    pub repetitions: usize,
    pub warmup: usize,
    /// The case ids of `redline-testing list --format json`, when given.
    pub case_manifest: Option<BTreeSet<String>>,
}

/// What a complete run contains.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunValidation {
    pub records: usize,
    pub cases: usize,
    pub executed_cases: usize,
    pub skipped_cases: usize,
    pub not_run_cases: usize,
    /// Cases with any failed row, placeholders included.
    pub failed_cases: usize,
    pub raw_sha256: String,
}

impl RunValidation {
    pub fn render(&self, plan: &RunPlan) -> String {
        format!(
            "validate-run: complete: {} cases ({} executed, {} skipped, {} not run; {} failed), {} records, {} warmup + {} measured repetitions per executed case, raw sha256 {}\n",
            self.cases,
            self.executed_cases,
            self.skipped_cases,
            self.not_run_cases,
            self.failed_cases,
            self.records,
            plan.warmup,
            plan.repetitions,
            self.raw_sha256
        )
    }
}

/// `<raw>.complete.json`, as the runner names it.
pub fn completion_marker_path(raw: &Path) -> PathBuf {
    let name = raw
        .file_name()
        .map_or_else(|| "raw".into(), |name| name.to_string_lossy().into_owned());
    raw.with_file_name(format!("{name}.complete.json"))
}

pub fn validate_run_path(raw: &Path, plan: &RunPlan) -> Result<RunValidation> {
    let bytes = fs::read(raw).with_context(|| format!("read raw records {}", raw.display()))?;
    if bytes.is_empty() {
        bail!("{} is empty: the runner wrote no records", raw.display());
    }
    let rows =
        parse_rows(Cursor::new(&bytes)).with_context(|| format!("parse {}", raw.display()))?;
    let mut validation = validate_rows(&rows, plan)
        .with_context(|| format!("{} is not the requested run", raw.display()))?;
    validation.raw_sha256 = format!("{:x}", Sha256::digest(&bytes));
    check_completion_marker(raw, &validation)?;
    Ok(validation)
}

#[derive(Default)]
struct CaseSamples {
    first_line: BTreeMap<SampleKey, usize>,
    warmups: usize,
    measured: BTreeSet<usize>,
    placeholder: Option<RowStatus>,
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
            SampleKey::Placeholder => case.placeholder = Some(row.status),
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
        match case.placeholder {
            Some(status) => {
                if case.first_line.len() != 1 {
                    bail!(
                        "case {case_id}: a case that did not run has one placeholder row, found {} rows",
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
        SampleKey::Placeholder => {}
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

#[derive(Deserialize)]
struct CompletionMarker {
    schema_version: String,
    raw_file: String,
    records: usize,
    cases: usize,
    raw_sha256: String,
}

fn check_completion_marker(raw: &Path, validation: &RunValidation) -> Result<()> {
    let path = completion_marker_path(raw);
    let text = fs::read_to_string(&path).with_context(|| {
        format!(
            "the runner wrote no completion marker {}: the run did not finish",
            path.display()
        )
    })?;
    let marker = serde_json::from_str::<CompletionMarker>(&text)
        .with_context(|| format!("parse completion marker {}", path.display()))?;
    let raw_name = raw
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    for (field, found, expected) in [
        (
            "schema_version",
            marker.schema_version.clone(),
            COMPLETION_SCHEMA.to_owned(),
        ),
        ("raw_file", marker.raw_file.clone(), raw_name),
        (
            "raw_sha256",
            marker.raw_sha256.clone(),
            validation.raw_sha256.clone(),
        ),
        (
            "records",
            marker.records.to_string(),
            validation.records.to_string(),
        ),
        (
            "cases",
            marker.cases.to_string(),
            validation.cases.to_string(),
        ),
    ] {
        if found != expected {
            bail!(
                "completion marker {} records {field} {found:?}, the raw file has {expected:?}",
                path.display()
            );
        }
    }
    Ok(())
}

/// The case ids of a `redline-testing list --format json` listing, as the
/// five-digit display ids raw records carry.
pub fn read_case_manifest(path: &Path) -> Result<BTreeSet<String>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read case manifest {}", path.display()))?;
    parse_case_manifest(&text).with_context(|| format!("case manifest {}", path.display()))
}

pub(crate) fn parse_case_manifest(text: &str) -> Result<BTreeSet<String>> {
    let Value::Array(cases) = serde_json::from_str::<Value>(text).context("not JSON")? else {
        bail!("not a JSON array of cases");
    };
    let mut ids = BTreeSet::new();
    for (index, case) in cases.iter().enumerate() {
        let id = case
            .get("id")
            .and_then(Value::as_u64)
            .with_context(|| format!("case {index} has no numeric id"))?;
        if !ids.insert(format!("{id:05}")) {
            bail!("case id {id:05} is listed twice");
        }
    }
    if ids.is_empty() {
        bail!("lists no case");
    }
    Ok(ids)
}

#[cfg(test)]
#[path = "validate_run_tests.rs"]
mod tests;
