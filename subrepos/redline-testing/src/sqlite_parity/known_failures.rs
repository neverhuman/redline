//! The SQLite known-failures baseline: every `sqlite_parity` and `memory`
//! case the current target is known to fail, with the verdict it fails with,
//! why, and who closes it.
//!
//! Listed failures are published as failures. The gate ratchets both ways,
//! like `metadata/beyond_sqlite/postgres-regression.json`: a failure the
//! baseline does not list is fatal, and so is a listed case that now passes
//! (remove it from the baseline in the commit whose run shows the pass).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::case::Case;
use super::runner::{RunSummary, VerdictReason};

pub const KNOWN_FAILURES_SCHEMA: &str = "redline-testing-sqlite-known-failures-v1";

/// The suites a baseline may list. Both run the same corpus through the
/// same comparator; `rql_phase1` has no baseline, so any failure there is
/// fatal.
pub const BASELINE_SUITES: [&str; 2] = ["sqlite_parity", "memory"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BaselineFile {
    schema_version: String,
    description: String,
    failures: Vec<KnownFailure>,
}

/// One listed failure.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnownFailure {
    pub suite: String,
    pub case_id: String,
    pub name: String,
    /// The `verdict_reason` every failing sample of the case records:
    /// `target_semantic_failure` or `differential_mismatch`. A reference
    /// contract failure is a corpus defect to re-bless, never a baseline
    /// entry.
    pub stage: VerdictReason,
    pub reason: String,
    pub owner: String,
}

/// Where a baseline was read from, as the evidence records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineSource {
    pub path: PathBuf,
    pub sha256: String,
}

/// A parsed baseline, or none (every failure is then fatal).
#[derive(Debug, Clone, Default)]
pub struct KnownFailures {
    source: Option<BaselineSource>,
    entries: BTreeMap<(String, String), KnownFailure>,
}

impl KnownFailures {
    /// No baseline: any failure fails the gate.
    pub fn none() -> Self {
        Self::default()
    }

    pub fn load(path: &Path) -> Result<Self> {
        let bytes = fs::read(path)
            .with_context(|| format!("read known-failures baseline {}", path.display()))?;
        Self::parse(&bytes, path)
            .with_context(|| format!("invalid known-failures baseline {}", path.display()))
    }

    fn parse(bytes: &[u8], path: &Path) -> Result<Self> {
        let file: BaselineFile = serde_json::from_slice(bytes)?;
        if file.schema_version != KNOWN_FAILURES_SCHEMA {
            bail!(
                "schema_version {:?}, expected {KNOWN_FAILURES_SCHEMA:?}",
                file.schema_version
            );
        }
        if file.description.trim().is_empty() {
            bail!("description is empty");
        }
        let mut entries = BTreeMap::new();
        for entry in file.failures {
            let label = format!("{} case {:?}", entry.suite, entry.case_id);
            if !BASELINE_SUITES.contains(&entry.suite.as_str()) {
                bail!("{label}: suite must be one of {BASELINE_SUITES:?}");
            }
            if entry.case_id.len() != 5 || !entry.case_id.bytes().all(|b| b.is_ascii_digit()) {
                bail!("{label}: case_id must be the five-digit display id");
            }
            if !matches!(
                entry.stage,
                VerdictReason::TargetSemanticFailure | VerdictReason::DifferentialMismatch
            ) {
                bail!(
                    "{label}: stage {} is not a target failure (target_semantic_failure or differential_mismatch)",
                    entry.stage.as_str()
                );
            }
            for (field, value) in [
                ("name", &entry.name),
                ("reason", &entry.reason),
                ("owner", &entry.owner),
            ] {
                if value.trim().is_empty() {
                    bail!("{label}: {field} is empty");
                }
            }
            let key = (entry.suite.clone(), entry.case_id.clone());
            if entries.insert(key, entry).is_some() {
                bail!("{label} is listed twice");
            }
        }
        Ok(Self {
            source: Some(BaselineSource {
                path: path.to_path_buf(),
                sha256: format!("{:x}", Sha256::digest(bytes)),
            }),
            entries,
        })
    }

    pub fn source(&self) -> Option<&BaselineSource> {
        self.source.as_ref()
    }

    /// Every entry names a corpus case by its id and name.
    pub fn check_corpus(&self, cases: &[Case]) -> Result<()> {
        let names = cases
            .iter()
            .map(|case| (case.display_id(), case.name.as_str()))
            .collect::<BTreeMap<_, _>>();
        for entry in self.entries.values() {
            match names.get(&entry.case_id) {
                Some(name) if *name == entry.name => {}
                Some(name) => bail!(
                    "{} case {} is named {name} in the corpus, not {}",
                    entry.suite,
                    entry.case_id,
                    entry.name
                ),
                None => bail!(
                    "{} case {} {} is not in the corpus",
                    entry.suite,
                    entry.case_id,
                    entry.name
                ),
            }
        }
        Ok(())
    }

    /// The case ids listed for `suite`.
    pub fn listed(&self, suite: &str) -> BTreeSet<String> {
        self.entries
            .keys()
            .filter(|(listed_suite, _)| listed_suite == suite)
            .map(|(_, case_id)| case_id.clone())
            .collect()
    }

    /// Passes when the suite's failed cases are exactly the listed ones,
    /// each failing with its listed verdict. Everything wrong is reported
    /// at once.
    pub fn gate(&self, suite: &str, summary: &RunSummary) -> Result<()> {
        let mut problems = Vec::new();
        let failed = summary
            .failures
            .iter()
            .map(|failure| (failure.case_id.as_str(), failure))
            .collect::<BTreeMap<_, _>>();
        if failed.len() != summary.failed {
            problems.push(format!(
                "the run counted {} failed cases but listed {}",
                summary.failed,
                failed.len()
            ));
        }
        for (case_id, failure) in &failed {
            let reasons = failure
                .verdict_reasons
                .iter()
                .map(|reason| reason.as_str())
                .collect::<Vec<_>>()
                .join("+");
            match self.entries.get(&(suite.to_owned(), (*case_id).to_owned())) {
                None => problems.push(format!(
                    "{case_id} {} failed ({reasons}) and is not in the baseline: fix it, or list it with a reason",
                    failure.name
                )),
                Some(entry)
                    if failure.verdict_reasons != BTreeSet::from([entry.stage]) =>
                {
                    problems.push(format!(
                        "{case_id} {} is listed as {} but failed as {reasons}: review the entry",
                        failure.name,
                        entry.stage.as_str()
                    ))
                }
                Some(_) => {}
            }
        }
        let skipped = summary
            .skipped_case_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        for ((listed_suite, case_id), entry) in &self.entries {
            if listed_suite != suite || failed.contains_key(case_id.as_str()) {
                continue;
            }
            if skipped.contains(case_id.as_str()) {
                problems.push(format!(
                    "{case_id} {} is listed as failing but was skipped, so the run cannot confirm it",
                    entry.name
                ));
            } else {
                problems.push(format!(
                    "{case_id} {} is listed as failing but passed: remove it from the baseline",
                    entry.name
                ));
            }
        }
        if problems.is_empty() {
            return Ok(());
        }
        let baseline = self
            .source
            .as_ref()
            .map(|source| source.path.display().to_string())
            .unwrap_or_else(|| "no known-failures baseline".to_owned());
        bail!(
            "{suite}: {} of {} cases failed, and the run does not match {baseline}:\n  - {}",
            summary.failed,
            summary.total,
            problems.join("\n  - ")
        )
    }
}

#[cfg(test)]
#[path = "known_failures_tests.rs"]
mod tests;
