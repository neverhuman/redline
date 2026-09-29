//! The SQLite known-failures baseline, as the evidence processor checks it.
//!
//! `sqlite_parity` and `memory` publish their failures. A run is accepted
//! only when each suite's failed cases, recomputed from its raw records,
//! are exactly the cases `metadata/sqlite_parity/known-failures.json` lists
//! for it, each failing with exactly the verdict_reason the baseline lists
//! as its stage, and the run recorded that same baseline file. An unlisted
//! failure, a listed case that passed and a listed case that failed another
//! way are all rejected: the runner writes its evidence before its own gate
//! judges it, so evidence from a run that gate rejected must not pass here.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(crate) const BASELINE_PATH: &str = "metadata/sqlite_parity/known-failures.json";
const BASELINE_SCHEMA: &str = "redline-testing-sqlite-known-failures-v1";
pub(crate) const BASELINE_SUITES: [&str; 2] = ["sqlite_parity", "memory"];

/// The stages a baseline entry may list: target failures only.
const BASELINE_STAGES: [&str; 2] = ["target_semantic_failure", "differential_mismatch"];

/// Each failed case of a raw file and the verdict_reasons its failed
/// records carry (`unrecorded` for a record with none).
pub(crate) type RawFailures = BTreeMap<String, BTreeSet<String>>;

/// The committed baseline: its hash, the case ids it lists per suite, and
/// the stage of each.
#[derive(Debug)]
pub(crate) struct Baseline {
    pub(crate) sha256: String,
    pub(crate) by_suite: BTreeMap<String, BTreeSet<String>>,
    stages: BTreeMap<(String, String), String>,
}

impl Baseline {
    pub(crate) fn load(repo_root: &Path) -> Result<Self> {
        let path = repo_root.join(BASELINE_PATH);
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&bytes).with_context(|| format!("invalid {}", path.display()))
    }

    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        let value: Value = serde_json::from_slice(bytes)?;
        if value["schema_version"] != BASELINE_SCHEMA {
            bail!(
                "schema_version {:?}, expected {BASELINE_SCHEMA:?}",
                value["schema_version"]
            );
        }
        let mut by_suite = BASELINE_SUITES
            .iter()
            .map(|suite| ((*suite).to_owned(), BTreeSet::new()))
            .collect::<BTreeMap<_, _>>();
        let mut stages = BTreeMap::new();
        let failures = value["failures"]
            .as_array()
            .ok_or_else(|| anyhow!("failures is not an array"))?;
        for entry in failures {
            let field = |name: &str| {
                entry[name]
                    .as_str()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| anyhow!("entry {entry} has no {name}"))
            };
            let suite = field("suite")?;
            let case_id = field("case_id")?;
            for name in ["name", "reason", "owner"] {
                field(name)?;
            }
            let stage = field("stage")?;
            if !BASELINE_STAGES.contains(&stage) {
                bail!(
                    "{suite} case {case_id} lists stage {stage:?}, not one of {BASELINE_STAGES:?}"
                );
            }
            let listed = by_suite
                .get_mut(suite)
                .ok_or_else(|| anyhow!("entry {entry} names suite {suite:?}"))?;
            if !listed.insert(case_id.to_owned()) {
                bail!("{suite} case {case_id} is listed twice");
            }
            stages.insert((suite.to_owned(), case_id.to_owned()), stage.to_owned());
        }
        Ok(Self {
            sha256: format!("{:x}", Sha256::digest(bytes)),
            by_suite,
            stages,
        })
    }

    fn listed(&self, suite: &str) -> &BTreeSet<String> {
        static EMPTY: BTreeSet<String> = BTreeSet::new();
        self.by_suite.get(suite).unwrap_or(&EMPTY)
    }

    /// The run was gated with this exact file.
    pub(crate) fn check_recorded(&self, official: &Value) -> Result<()> {
        let recorded = &official["sqlite_known_failures"];
        if recorded["schema_version"] != BASELINE_SCHEMA {
            bail!("official evidence records no SQLite known-failures baseline: {recorded}");
        }
        if recorded["sha256"].as_str() != Some(self.sha256.as_str()) {
            bail!(
                "the run was gated with known-failures sha256 {}, but {BASELINE_PATH} is {}",
                recorded["sha256"],
                self.sha256
            );
        }
        Ok(())
    }

    /// The suite's failures, as its raw records show them, the runner's
    /// summary declares them and the baseline lists them, are one set, and
    /// every failed record of a listed case carries the listed stage.
    pub(crate) fn check_suite(
        &self,
        suite: &str,
        entry: &Value,
        raw_failures: &RawFailures,
    ) -> Result<()> {
        let raw_failed = &raw_failures.keys().cloned().collect::<BTreeSet<_>>();
        let listed = self.listed(suite);
        let declared = id_set(entry, "failed_case_ids")?;
        let recorded_listed = id_set(entry, "known_failure_ids")?;
        let failed = entry["failed"]
            .as_u64()
            .ok_or_else(|| anyhow!("suite {suite} has no failed count"))?;
        if usize::try_from(failed).ok() != Some(raw_failed.len()) {
            bail!(
                "suite {suite} counts {failed} failed cases, but its raw records fail {}",
                raw_failed.len()
            );
        }
        if &declared != raw_failed {
            bail!(
                "suite {suite} declares failed cases {declared:?}, but its raw records fail {raw_failed:?}"
            );
        }
        if &recorded_listed != listed {
            bail!(
                "suite {suite} was gated with known failures {recorded_listed:?}, but {BASELINE_PATH} lists {listed:?}"
            );
        }
        let unlisted = raw_failed.difference(listed).collect::<Vec<_>>();
        let passing = listed.difference(raw_failed).collect::<Vec<_>>();
        if !unlisted.is_empty() || !passing.is_empty() {
            bail!(
                "suite {suite} failures differ from {BASELINE_PATH}: failed but not listed {unlisted:?}; listed but not failed {passing:?} (remove those from the baseline)"
            );
        }
        for (case_id, reasons) in raw_failures {
            let stage = self
                .stages
                .get(&(suite.to_owned(), case_id.clone()))
                .ok_or_else(|| anyhow!("suite {suite} case {case_id} has no listed stage"))?;
            if reasons.len() != 1 || !reasons.contains(stage) {
                bail!(
                    "suite {suite} case {case_id} is listed as {stage}, but its raw records fail as {reasons:?}"
                );
            }
        }
        Ok(())
    }

    /// What the processed evidence records about the baseline.
    pub(crate) fn summary(&self) -> Value {
        json!({
            "path": BASELINE_PATH,
            "schema_version": BASELINE_SCHEMA,
            "sha256": self.sha256,
            "suites": self.by_suite,
        })
    }
}

fn id_set(entry: &Value, key: &str) -> Result<BTreeSet<String>> {
    entry[key]
        .as_array()
        .ok_or_else(|| anyhow!("suite entry has no {key} list"))?
        .iter()
        .map(|id| {
            id.as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("{key} holds a non-string id {id}"))
        })
        .collect()
}

/// The case ids with a failed record in a raw JSONL file, each with the
/// verdict_reasons of its failed records.
pub(crate) fn raw_failures(path: &Path) -> Result<RawFailures> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut failed = RawFailures::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(line)
            .with_context(|| format!("parse {} line {}", path.display(), index + 1))?;
        if record["status"] == "failed" {
            let case_id = record["case_id"]
                .as_str()
                .ok_or_else(|| anyhow!("{} line {} has no case_id", path.display(), index + 1))?;
            let reason = record["verdict_reason"].as_str().unwrap_or("unrecorded");
            failed
                .entry(case_id.to_owned())
                .or_default()
                .insert(reason.to_owned());
        }
    }
    Ok(failed)
}

#[cfg(test)]
#[path = "sqlite_known_failures_tests.rs"]
mod tests;
