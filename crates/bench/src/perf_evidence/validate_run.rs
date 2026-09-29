//! `validate-run`: is a runner's raw file the complete experiment that was
//! asked for? (BM3-01)
//!
//! A perf lane may summarize a run only after this check passes. Every row
//! must parse (`records::parse_rows`) and be one sample of the run's plan:
//! each executed case has exactly `warmup` warmups and measured
//! repetitions `1..=repetitions`, each once; a case that did not run is one
//! not-run row. The cases must be exactly `expected_cases` in number
//! and, given the runner's own corpus listing, exactly its ids. The
//! runner's completion marker (`<raw>.complete.json`) must exist and name
//! this file's SHA-256, record count and case count: the runner writes it
//! only when a suite finished, so an interrupted run never validates.
//!
//! The check is about completeness, not correctness: failed cases are
//! counted and left to the lane's known-failures filter.

mod rows;

use std::collections::BTreeSet;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::records::parse_rows;

pub(crate) use rows::validate_rows;

pub const COMPLETION_SCHEMA: &str = "redline-testing-raw-complete-v1";

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
    /// Cases with any failed row, not-run rows included.
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
