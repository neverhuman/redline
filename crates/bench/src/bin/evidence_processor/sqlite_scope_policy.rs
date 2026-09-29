//! The SQLite scope policy, as the evidence processor checks it (SQ-05).
//!
//! There is no skip budget. A `sqlite_parity`, `memory` or `rql_phase1`
//! run is accepted only when each suite's skipped cases, recomputed from its
//! raw records, are exactly the cases the committed
//! `subrepos/redline-testing/corpus/sqlite_parity/scope-policy.json` lists
//! for it, each record names its exception, the runner declared the same
//! ids, and the run was selected with that same policy file.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(crate) const POLICY_PATH: &str =
    "subrepos/redline-testing/corpus/sqlite_parity/scope-policy.json";
const POLICY_SCHEMA: &str = "redline-testing-sqlite-scope-policy-v1";
pub(crate) const POLICY_SUITES: [&str; 3] = ["sqlite_parity", "memory", "rql_phase1"];

/// The committed policy: its hash and the case ids it lists per suite.
#[derive(Debug)]
pub(crate) struct ScopePolicy {
    pub(crate) sha256: String,
    pub(crate) by_suite: BTreeMap<String, BTreeSet<String>>,
}

impl ScopePolicy {
    pub(crate) fn load(repo_root: &Path) -> Result<Self> {
        let path = repo_root.join(POLICY_PATH);
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&bytes).with_context(|| format!("invalid {}", path.display()))
    }

    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        let value: Value = serde_json::from_slice(bytes)?;
        if value["schema_version"] != POLICY_SCHEMA {
            bail!(
                "schema_version {}, expected {POLICY_SCHEMA:?}",
                value["schema_version"]
            );
        }
        let mut by_suite = POLICY_SUITES
            .iter()
            .map(|suite| ((*suite).to_owned(), BTreeSet::new()))
            .collect::<BTreeMap<_, _>>();
        let exceptions = value["exceptions"]
            .as_array()
            .ok_or_else(|| anyhow!("exceptions is not an array"))?;
        for entry in exceptions {
            let field = |name: &str| {
                entry[name]
                    .as_str()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| anyhow!("exception {entry} has no {name}"))
            };
            let suite = field("suite")?;
            let case_id = field("case_id")?;
            for name in ["name", "kind", "reason", "owner", "expiry"] {
                field(name)?;
            }
            let listed = by_suite
                .get_mut(suite)
                .ok_or_else(|| anyhow!("exception {entry} names suite {suite:?}"))?;
            if !listed.insert(case_id.to_owned()) {
                bail!("{suite} case {case_id} is listed twice");
            }
        }
        Ok(Self {
            sha256: format!("{:x}", Sha256::digest(bytes)),
            by_suite,
        })
    }

    fn listed(&self, suite: &str) -> &BTreeSet<String> {
        static EMPTY: BTreeSet<String> = BTreeSet::new();
        self.by_suite.get(suite).unwrap_or(&EMPTY)
    }

    /// The run selected its cases with this exact file.
    pub(crate) fn check_recorded(&self, official: &Value) -> Result<()> {
        let recorded = &official["sqlite_scope_policy"];
        if recorded["schema_version"] != POLICY_SCHEMA {
            bail!("official evidence records no SQLite scope policy: {recorded}");
        }
        if recorded["sha256"].as_str() != Some(self.sha256.as_str()) {
            bail!(
                "the run was selected with scope policy sha256 {}, but {POLICY_PATH} is {}",
                recorded["sha256"],
                self.sha256
            );
        }
        Ok(())
    }

    /// The suite's skips, as its raw records show them, as the runner
    /// declared them, and as the policy lists them, are one set.
    pub(crate) fn check_suite(
        &self,
        suite: &str,
        entry: &Value,
        raw_skipped: &BTreeSet<String>,
    ) -> Result<()> {
        let listed = self.listed(suite);
        let skipped = entry["skipped"]
            .as_u64()
            .ok_or_else(|| anyhow!("suite {suite} has no skipped count"))?;
        if usize::try_from(skipped).ok() != Some(raw_skipped.len()) {
            bail!(
                "suite {suite} counts {skipped} skipped cases, but its raw records skip {}",
                raw_skipped.len()
            );
        }
        let declared = entry["skipped_case_ids"]
            .as_array()
            .ok_or_else(|| anyhow!("suite {suite} declares no skipped_case_ids"))?
            .iter()
            .map(|id| {
                id.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| anyhow!("skipped_case_ids holds a non-string id {id}"))
            })
            .collect::<Result<BTreeSet<_>>>()?;
        if &declared != raw_skipped {
            bail!(
                "suite {suite} declares skipped cases {declared:?}, but its raw records skip {raw_skipped:?}"
            );
        }
        let unlisted = raw_skipped.difference(listed).collect::<Vec<_>>();
        let ran = listed.difference(raw_skipped).collect::<Vec<_>>();
        if !unlisted.is_empty() || !ran.is_empty() {
            bail!(
                "suite {suite} skips differ from {POLICY_PATH}: skipped but not listed {unlisted:?}; listed but not skipped {ran:?}"
            );
        }
        Ok(())
    }

    /// What the processed evidence records about the policy.
    pub(crate) fn summary(&self) -> Value {
        json!({
            "path": POLICY_PATH,
            "schema_version": POLICY_SCHEMA,
            "sha256": self.sha256,
            "suites": self.by_suite,
        })
    }
}

/// The case ids a raw JSONL file skips. Each skip must be one placeholder
/// record naming the exception it was skipped under (`<suite>:<case_id>`).
pub(crate) fn raw_skipped_case_ids(path: &Path, suite: &str) -> Result<BTreeSet<String>> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut skipped = BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(line)
            .with_context(|| format!("parse {} line {}", path.display(), index + 1))?;
        if record["status"] != "skipped" {
            continue;
        }
        let case_id = record["case_id"]
            .as_str()
            .ok_or_else(|| anyhow!("{} line {} has no case_id", path.display(), index + 1))?;
        let exception = format!("{suite}:{case_id}");
        if record["policy_exception_id"].as_str() != Some(exception.as_str()) {
            bail!(
                "{} line {}: skipped case {case_id} names exception {}, expected {exception:?}",
                path.display(),
                index + 1,
                record["policy_exception_id"]
            );
        }
        if !skipped.insert(case_id.to_owned()) {
            bail!("{} skips case {case_id} twice", path.display());
        }
    }
    Ok(skipped)
}

#[cfg(test)]
#[path = "sqlite_scope_policy_tests.rs"]
mod tests;
