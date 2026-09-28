//! Strict parsing of the redline-testing runner's raw JSONL rows.
//!
//! Every line must be one JSON object describing one known sample: a
//! warmup, `measured:K` with `repetition_index` K, or the single
//! not-run row of a case that did not run (`skipped`/`skipped` or
//! `not_run`/`failed`). Anything else is an error naming its line, never a
//! row to skip: a summary built from the rows a parser happened to
//! understand describes an experiment nobody ran.

use std::io::BufRead;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value};

/// A row's verdict as the runner recorded it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowStatus {
    Passed,
    Failed,
    Skipped,
}

/// A sample's identity within its case; each may occur once per case.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SampleKey {
    /// A warmup, by its `sample_index` when the row carries one.
    Warmup(Option<usize>),
    /// Measured repetition K (1-based).
    Measured(usize),
    /// The one row of a case that did not run.
    NotRun,
}

impl std::fmt::Display for SampleKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Warmup(Some(index)) => write!(f, "warmup (sample {index})"),
            Self::Warmup(None) => f.write_str("warmup"),
            Self::Measured(repetition) => write!(f, "measured:{repetition}"),
            Self::NotRun => f.write_str("not_run"),
        }
    }
}

/// The two engine durations of an executed sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Timing {
    pub(crate) reference_ns: u64,
    pub(crate) target_ns: u64,
    pub(crate) latency_ratio: Option<f64>,
}

impl Timing {
    /// Why these durations cannot form a ratio, if they cannot: a zero
    /// duration, or a recorded ratio that is not target / reference.
    pub(crate) fn defect(&self) -> Option<String> {
        if self.reference_ns == 0 || self.target_ns == 0 {
            return Some(format!(
                "zero duration (reference {} ns, target {} ns)",
                self.reference_ns, self.target_ns
            ));
        }
        let ratio = self.ratio();
        match self.latency_ratio {
            Some(recorded) if !recorded.is_finite() || recorded <= 0.0 => {
                Some(format!("latency_ratio {recorded} is not a positive number"))
            }
            Some(recorded) if ((recorded - ratio) / ratio).abs() > 1e-9 => Some(format!(
                "latency_ratio {recorded} is not target/reference {ratio}"
            )),
            _ => None,
        }
    }

    pub(crate) fn ratio(&self) -> f64 {
        self.target_ns as f64 / self.reference_ns as f64
    }
}

/// One parsed row and the line it came from (1-based).
#[derive(Clone, Debug)]
pub(crate) struct RawRow {
    pub(crate) line: usize,
    pub(crate) case_id: String,
    pub(crate) status: RowStatus,
    pub(crate) sample: SampleKey,
    /// Present on every warmup and measured row.
    pub(crate) timing: Option<Timing>,
    /// The row's `target_executable_sha256` and `reference_executable_sha256`
    /// (the binaries the runner timed); `None` when absent or empty, as on
    /// the not-run row of a case that did not run.
    pub(crate) target_sha256: Option<String>,
    pub(crate) reference_sha256: Option<String>,
}

/// Parses every line of `reader`; the first line that is not a known
/// sample is an error naming it.
pub(crate) fn parse_rows(reader: impl BufRead) -> Result<Vec<RawRow>> {
    let mut rows = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let number = index + 1;
        let line = line.with_context(|| format!("read JSONL line {number}"))?;
        rows.push(parse_row(&line, number).with_context(|| format!("JSONL line {number}"))?);
    }
    Ok(rows)
}

fn parse_row(line: &str, number: usize) -> Result<RawRow> {
    let value = serde_json::from_str::<Value>(line).context("not JSON")?;
    let Value::Object(row) = value else {
        bail!("not a JSON object");
    };
    let case_id = string_field(&row, "case_id")?;
    if case_id.trim().is_empty() {
        bail!("case_id is empty");
    }
    let status = match string_field(&row, "status")?.as_str() {
        "passed" => RowStatus::Passed,
        "failed" => RowStatus::Failed,
        "skipped" => RowStatus::Skipped,
        other => bail!("status {other:?} is not passed, failed or skipped"),
    };
    let role = string_field(&row, "sample_role")?;
    let sample_index = optional_index(&row, "sample_index")?;
    let repetition_index = optional_index(&row, "repetition_index")?;
    let sample = if role == "warmup" {
        if repetition_index.is_some() {
            bail!("warmup carries repetition_index {repetition_index:?}");
        }
        SampleKey::Warmup(sample_index)
    } else if let Some(repetition) = role.strip_prefix("measured:") {
        let repetition = repetition
            .parse::<usize>()
            .ok()
            .filter(|repetition| *repetition >= 1)
            .ok_or_else(|| anyhow!("sample_role {role:?} names no repetition"))?;
        if repetition_index != Some(repetition) {
            bail!("{role} carries repetition_index {repetition_index:?}");
        }
        SampleKey::Measured(repetition)
    } else if (role == "skipped" && status == RowStatus::Skipped)
        || (role == "not_run" && status == RowStatus::Failed)
    {
        SampleKey::NotRun
    } else {
        bail!("sample_role {role:?} with status {status:?} is no known sample");
    };
    let timing = match sample {
        SampleKey::NotRun => None,
        SampleKey::Warmup(_) | SampleKey::Measured(_) => {
            if status == RowStatus::Skipped {
                bail!("{sample} has status skipped");
            }
            Some(Timing {
                reference_ns: duration_field(&row, "reference_elapsed_ns")?,
                target_ns: duration_field(&row, "target_elapsed_ns")?,
                latency_ratio: match row.get("latency_ratio") {
                    None | Some(Value::Null) => None,
                    Some(value) => Some(
                        value
                            .as_f64()
                            .ok_or_else(|| anyhow!("latency_ratio {value} is not a number"))?,
                    ),
                },
            })
        }
    };
    Ok(RawRow {
        line: number,
        case_id,
        status,
        sample,
        timing,
        target_sha256: optional_digest(&row, "target_executable_sha256")?,
        reference_sha256: optional_digest(&row, "reference_executable_sha256")?,
    })
}

fn string_field(row: &Map<String, Value>, name: &str) -> Result<String> {
    match row.get(name) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(other) => bail!("{name} {other} is not a string"),
        None => bail!("missing {name}"),
    }
}

fn optional_index(row: &Map<String, Value>, name: &str) -> Result<Option<usize>> {
    match row.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|index| usize::try_from(index).ok())
            .map(Some)
            .ok_or_else(|| anyhow!("{name} {value} is not a non-negative integer")),
    }
}

fn duration_field(row: &Map<String, Value>, name: &str) -> Result<u64> {
    match row.get(name) {
        Some(value) => value
            .as_u64()
            .ok_or_else(|| anyhow!("{name} {value} is not a non-negative integer")),
        None => bail!("missing {name}"),
    }
}

/// A digest field that may be absent or empty, but is a string when set.
fn optional_digest(row: &Map<String, Value>, name: &str) -> Result<Option<String>> {
    match row.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.is_empty() => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(other) => bail!("{name} {other} is not a string"),
    }
}
