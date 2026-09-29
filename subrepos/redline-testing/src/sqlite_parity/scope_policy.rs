//! The exact skip policy of the SQLite-shell suites (SQ-05).
//!
//! A case is skipped only when `corpus/sqlite_parity/scope-policy.json`
//! lists it for the suite, with the gap it covers, why, who closes it and
//! when the exception expires. Every other gap fails its case: a reference
//! shell without a capability the case declares is
//! `reference_capability_missing`, and a target without one, or an RQL
//! phase-1 rewrite that cannot express the case, is `target_unsupported`.
//! A listed case that runs fails the gate, so the list shrinks with the
//! gaps, and the evidence processor holds every published skip to exactly
//! this list. There is no skip budget.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::case::Case;
use super::runner::RunSummary;

pub const SCOPE_POLICY_SCHEMA: &str = "redline-testing-sqlite-scope-policy-v1";
/// Where the policy compiled into this runner lives in its source tree.
pub const SCOPE_POLICY_PATH: &str = "corpus/sqlite_parity/scope-policy.json";
const POLICY_SUITES: [&str; 3] = ["sqlite_parity", "memory", "rql_phase1"];
const COMPILED_POLICY: &str = include_str!("../../corpus/sqlite_parity/scope-policy.json");

/// The gap an exception covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GapKind {
    /// The target lacks a capability the case requires.
    TargetCapability,
    /// The RQL phase-1 rewrite cannot express the case.
    RqlRewrite,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    schema_version: String,
    description: String,
    exceptions: Vec<Exception>,
}

/// One listed skip.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exception {
    pub suite: String,
    pub case_id: String,
    pub name: String,
    pub kind: GapKind,
    pub reason: String,
    pub owner: String,
    /// The last day (UTC, `YYYY-MM-DD`) an official run accepts the entry.
    pub expiry: String,
}

#[derive(Debug, Clone)]
pub struct ScopePolicy {
    sha256: String,
    entries: BTreeMap<(String, String), Exception>,
}

impl ScopePolicy {
    /// The policy compiled into this runner.
    pub fn compiled() -> Result<Self> {
        Self::parse(COMPILED_POLICY.as_bytes())
            .with_context(|| format!("invalid {SCOPE_POLICY_PATH}"))
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let file: PolicyFile = serde_json::from_slice(bytes)?;
        if file.schema_version != SCOPE_POLICY_SCHEMA {
            bail!(
                "schema_version {:?}, expected {SCOPE_POLICY_SCHEMA:?}",
                file.schema_version
            );
        }
        if file.description.trim().is_empty() {
            bail!("description is empty");
        }
        let mut entries = BTreeMap::new();
        for entry in file.exceptions {
            let label = format!("{} case {:?}", entry.suite, entry.case_id);
            if !POLICY_SUITES.contains(&entry.suite.as_str()) {
                bail!("{label}: suite must be one of {POLICY_SUITES:?}");
            }
            if entry.case_id.len() != 5 || !entry.case_id.bytes().all(|b| b.is_ascii_digit()) {
                bail!("{label}: case_id must be the five-digit display id");
            }
            if entry.kind == GapKind::RqlRewrite && entry.suite != "rql_phase1" {
                bail!("{label}: only rql_phase1 has an RQL rewrite");
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
            if !is_iso_date(&entry.expiry) {
                bail!(
                    "{label}: expiry {:?} is not a YYYY-MM-DD date",
                    entry.expiry
                );
            }
            let key = (entry.suite.clone(), entry.case_id.clone());
            if entries.insert(key, entry).is_some() {
                bail!("{label} is listed twice");
            }
        }
        Ok(Self {
            sha256: format!("{:x}", Sha256::digest(bytes)),
            entries,
        })
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// The id a skipped record carries for the exception that covers it.
    pub fn exception_id(suite: &str, case_id: &str) -> String {
        format!("{suite}:{case_id}")
    }

    /// The exception id covering `kind` of gap in `case` for `suite`.
    pub fn covers(&self, suite: &str, case: &Case, kind: GapKind) -> Option<String> {
        let entry = self.entries.get(&(suite.to_owned(), case.display_id()))?;
        (entry.kind == kind).then(|| Self::exception_id(suite, &entry.case_id))
    }

    /// The case ids listed for `suite`.
    pub fn listed(&self, suite: &str) -> BTreeSet<String> {
        self.entries
            .keys()
            .filter(|(listed_suite, _)| listed_suite == suite)
            .map(|(_, case_id)| case_id.clone())
            .collect()
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
                    "{SCOPE_POLICY_PATH}: {} case {} is named {name} in the corpus, not {}",
                    entry.suite,
                    entry.case_id,
                    entry.name
                ),
                None => bail!(
                    "{SCOPE_POLICY_PATH}: {} case {} {} is not in the corpus",
                    entry.suite,
                    entry.case_id,
                    entry.name
                ),
            }
        }
        Ok(())
    }

    /// An official run accepts no expired exception: each one is reviewed
    /// again, with a new expiry, or the gap is closed.
    pub fn check_expiry(&self, today: &str) -> Result<()> {
        let expired = self
            .entries
            .values()
            .filter(|entry| entry.expiry.as_str() < today)
            .map(|entry| {
                format!(
                    "{}:{} (expired {})",
                    entry.suite, entry.case_id, entry.expiry
                )
            })
            .collect::<Vec<_>>();
        if !expired.is_empty() {
            bail!(
                "{SCOPE_POLICY_PATH} has {} expired exception(s) on {today}: {}",
                expired.len(),
                expired.join(", ")
            );
        }
        Ok(())
    }

    /// Fails when a case the policy lists for `suite` was not skipped: it
    /// ran or failed, so its entry no longer describes the run.
    pub fn gate(&self, suite: &str, summary: &RunSummary) -> Result<()> {
        let ran = summary
            .passed_case_ids
            .iter()
            .chain(summary.failures.iter().map(|failure| &failure.case_id))
            .collect::<BTreeSet<_>>();
        let executed = self
            .listed(suite)
            .into_iter()
            .filter(|case_id| ran.contains(case_id))
            .collect::<Vec<_>>();
        if executed.is_empty() {
            return Ok(());
        }
        bail!(
            "{suite}: {SCOPE_POLICY_PATH} lists case(s) {} as skipped, but the run executed them: remove the entries",
            executed.join(", ")
        )
    }
}

fn is_iso_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<u32>().ok();
    matches!(
        (number(0..4), number(5..7), number(8..10)),
        (Some(_), Some(1..=12), Some(1..=31))
    )
}

/// Today's UTC date as `YYYY-MM-DD`.
pub fn today_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    civil_date(secs / 86_400)
}

/// The proleptic Gregorian date `days` after 1970-01-01 (H. Hinnant's
/// `civil_from_days`).
fn civil_date(days: u64) -> String {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
#[path = "scope_policy_tests.rs"]
mod tests;
