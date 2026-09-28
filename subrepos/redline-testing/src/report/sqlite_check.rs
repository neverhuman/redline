//! `check-sqlite` (SQ-04): the runner's own reduction of an official run's
//! SQLite-shell suites to one verdict per case, written as
//! `sqlite-qualification.json`. The RedlineDB evidence processor cannot
//! link this crate, so it requires this file instead of repeating the
//! reduction, and cross-checks it against the raw records.
//!
//! For each of `sqlite_parity`, `memory` and `rql_phase1` the raw records
//! must hold, for exactly the cases of the suite's compiled-in manifest,
//! every warmup and measured repetition of the run once, and the runner's
//! counts and failed and skipped ids must be what the reduction gives.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use super::render::parse_raw_records;
use super::utils::{is_measured, sha256_bytes, sha256_file};
use super::verdicts::reduce_sqlite_verdicts;

pub const QUALIFICATION_SCHEMA: &str = "redline-testing-sqlite-qualification-v1";
const RUN_EVIDENCE_SCHEMA: &str = "redline-testing-official-evidence-v1";
const SUITES: [&str; 3] = ["sqlite_parity", "memory", "rql_phase1"];

#[derive(Debug)]
pub struct CheckSqliteOptions {
    /// The run's `official-evidence.json`; raw paths are relative to it.
    pub official_evidence: PathBuf,
    pub output: PathBuf,
}

pub fn check_sqlite(options: CheckSqliteOptions) -> Result<()> {
    let evidence_bytes = fs::read(&options.official_evidence).with_context(|| {
        format!(
            "read official evidence {}",
            options.official_evidence.display()
        )
    })?;
    let evidence: Value = serde_json::from_slice(&evidence_bytes).with_context(|| {
        format!(
            "parse official evidence {}",
            options.official_evidence.display()
        )
    })?;
    if evidence["schema_version"] != RUN_EVIDENCE_SCHEMA {
        bail!(
            "check-sqlite reads a run's {RUN_EVIDENCE_SCHEMA}, not {}",
            evidence["schema_version"]
        );
    }
    let root = options
        .official_evidence
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let plan = |field: &str| {
        evidence[field]
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| anyhow!("official evidence records no {field}"))
    };
    let (warmup, repetitions) = (plan("warmup")?, plan("repetitions")?);
    let mut suites = BTreeMap::new();
    for suite in SUITES {
        let checked = check_suite(root, suite, &evidence["suites"][suite], warmup, repetitions)
            .with_context(|| format!("suite {suite}"))?;
        suites.insert(suite, checked);
    }
    let runner = std::env::current_exe().context("resolve current executable")?;
    let qualification = json!({
        "schema_version": QUALIFICATION_SCHEMA,
        "runner": {
            "version": format!("redline-testing {}", env!("CARGO_PKG_VERSION")),
            "binary_sha256": sha256_file(&runner)?,
        },
        "official_evidence_sha256": sha256_bytes(&evidence_bytes),
        "corpus_sha256": crate::sqlite_parity::corpus_sha256(),
        "warmup": warmup,
        "repetitions": repetitions,
        "suites": suites,
    });
    fs::write(
        &options.output,
        serde_json::to_string_pretty(&qualification)? + "\n",
    )
    .with_context(|| format!("write {}", options.output.display()))
}

fn check_suite(
    root: &Path,
    suite: &str,
    entry: &Value,
    warmup: usize,
    repetitions: usize,
) -> Result<Value> {
    let raw_path = entry["raw_path"]
        .as_str()
        .ok_or_else(|| anyhow!("official evidence declares no raw_path"))?;
    let raw_bytes = fs::read(root.join(raw_path)).with_context(|| format!("read {raw_path}"))?;
    let raw_text =
        String::from_utf8(raw_bytes.clone()).with_context(|| format!("{raw_path} is not UTF-8"))?;
    let records = parse_raw_records(&raw_text)?;
    let manifest = crate::sqlite_parity::manifest_case_ids(suite)?;
    let verdicts = reduce_sqlite_verdicts(&records, &manifest, warmup, repetitions)?;
    for (field, count) in [
        ("total", verdicts.total()),
        ("passed", verdicts.passed.len()),
        ("failed", verdicts.failed.len()),
        ("skipped", verdicts.skipped.len()),
    ] {
        if entry[field].as_u64() != Some(count as u64) {
            bail!(
                "the runner counted {field}={}, but its raw records reduce to {count}",
                entry[field]
            );
        }
    }
    for (field, ids) in [
        ("failed_case_ids", &verdicts.failed),
        ("skipped_case_ids", &verdicts.skipped),
    ] {
        let declared =
            id_set(&entry[field]).with_context(|| format!("official evidence {field}"))?;
        if &declared != ids {
            bail!(
                "the runner declared {field} {declared:?}, but its raw records reduce to {ids:?}"
            );
        }
    }
    let manifest_text = manifest.iter().cloned().collect::<Vec<_>>().join("\n");
    Ok(json!({
        "raw_path": raw_path,
        "raw_sha256": sha256_bytes(&raw_bytes),
        "manifest_sha256": sha256_bytes(manifest_text.as_bytes()),
        "total": verdicts.total(),
        "passed": verdicts.passed.len(),
        "failed": verdicts.failed.len(),
        "skipped": verdicts.skipped.len(),
        "passed_case_ids": verdicts.passed,
        "failed_case_ids": verdicts.failed,
        "skipped_case_ids": verdicts.skipped,
        "measured_samples": records.iter().filter(|record| is_measured(record)).count(),
        "warmup_samples": records.iter().filter(|record| record.sample_role == "warmup").count(),
    }))
}

fn id_set(value: &Value) -> Result<BTreeSet<String>> {
    value
        .as_array()
        .ok_or_else(|| anyhow!("is not a list"))?
        .iter()
        .map(|id| {
            id.as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("holds a non-string id {id}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(case_id: &str, role: &str, status: &str) -> String {
        let repetition = role
            .strip_prefix("measured:")
            .map(|k| k.parse::<usize>().unwrap());
        format!(
            "{}\n",
            json!({"case_id": case_id, "name": "N", "priority": "P0", "profile": "memory",
                   "category": "C", "sample_role": role, "repetition_index": repetition,
                   "status": status, "reference_elapsed_ns": 1, "target_elapsed_ns": 2})
        )
    }

    /// Every rql_phase1 case passed once, except the first, which failed.
    fn rql_run(dir: &Path) -> (Value, BTreeSet<String>) {
        let manifest = crate::sqlite_parity::manifest_case_ids("rql_phase1").expect("manifest");
        let first = manifest.iter().next().expect("a case").clone();
        let raw = manifest
            .iter()
            .map(|id| {
                record(
                    id,
                    "measured:1",
                    if *id == first { "failed" } else { "passed" },
                )
            })
            .collect::<String>();
        fs::write(dir.join("rql.raw.jsonl"), raw).expect("raw");
        let entry = json!({
            "raw_path": "rql.raw.jsonl", "total": manifest.len(),
            "passed": manifest.len() - 1, "failed": 1, "skipped": 0,
            "failed_case_ids": [first], "skipped_case_ids": [],
        });
        (entry, manifest)
    }

    #[test]
    fn a_suite_reduces_to_the_runners_own_counts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (entry, manifest) = rql_run(dir.path());
        let checked = check_suite(dir.path(), "rql_phase1", &entry, 0, 1).expect("consistent");
        assert_eq!(checked["total"], manifest.len());
        assert_eq!(checked["failed"], 1);
        assert_eq!(
            checked["passed_case_ids"].as_array().map(Vec::len),
            Some(manifest.len() - 1)
        );
        // A runner count, or a declared id list, the records do not give.
        for (field, value) in [
            ("passed", json!(manifest.len())),
            ("skipped", json!(1)),
            ("failed_case_ids", json!([])),
            ("skipped_case_ids", json!(null)),
        ] {
            let mut wrong = entry.clone();
            wrong[field] = value;
            assert!(
                check_suite(dir.path(), "rql_phase1", &wrong, 0, 1).is_err(),
                "{field}"
            );
        }
        // The sample plan is the run's: one repetition is not two.
        assert!(check_suite(dir.path(), "rql_phase1", &entry, 0, 2).is_err());
        // Another suite's manifest is not this one's.
        assert!(check_suite(dir.path(), "sqlite_parity", &entry, 0, 1).is_err());
    }
}
