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
mod tests {
    use super::*;

    fn policy(entries: &[(&str, &str)]) -> ScopePolicy {
        let exceptions = entries
            .iter()
            .map(|(suite, case_id)| {
                json!({"suite": suite, "case_id": case_id, "name": "N", "kind": "rql_rewrite",
                       "reason": "r", "owner": "o", "expiry": "2027-03-31"})
            })
            .collect::<Vec<_>>();
        let text =
            json!({"schema_version": POLICY_SCHEMA, "description": "d", "exceptions": exceptions});
        ScopePolicy::parse(text.to_string().as_bytes()).expect("policy")
    }

    fn entry(skipped: &[&str]) -> Value {
        json!({"skipped": skipped.len(), "skipped_case_ids": skipped})
    }

    fn ids(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    #[test]
    fn skip_ids_must_equal_policy() {
        let policy = policy(&[("rql_phase1", "00007"), ("rql_phase1", "00010")]);
        let listed = ids(&["00007", "00010"]);
        policy
            .check_suite("rql_phase1", &entry(&["00007", "00010"]), &listed)
            .expect("exactly the listed skips");
        // The old budget accepted four anonymous sqlite_parity skips.
        let four = ids(&["00093", "00094", "00095", "00096"]);
        let error = policy
            .check_suite(
                "sqlite_parity",
                &entry(&["00093", "00094", "00095", "00096"]),
                &four,
            )
            .expect_err("unlisted skips");
        assert!(
            format!("{error:#}").contains("skipped but not listed"),
            "{error:#}"
        );
        // A listed case that ran.
        let error = policy
            .check_suite("rql_phase1", &entry(&["00007"]), &ids(&["00007"]))
            .expect_err("listed case ran");
        assert!(
            format!("{error:#}").contains("listed but not skipped [\"00010\"]"),
            "{error:#}"
        );
        // One more skip than the policy allows.
        let three = ids(&["00007", "00010", "00011"]);
        assert!(
            policy
                .check_suite("rql_phase1", &entry(&["00007", "00010", "00011"]), &three)
                .is_err()
        );
        // The runner's declaration must match its raw records.
        assert!(
            policy
                .check_suite("rql_phase1", &entry(&["00007", "00011"]), &listed)
                .is_err()
        );
        let mut miscounted = entry(&["00007", "00010"]);
        miscounted["skipped"] = json!(3);
        assert!(
            policy
                .check_suite("rql_phase1", &miscounted, &listed)
                .is_err()
        );
        assert!(
            policy
                .check_suite("rql_phase1", &json!({"skipped": 2}), &listed)
                .is_err()
        );
    }

    #[test]
    fn the_run_must_record_the_committed_policy() {
        let policy = policy(&[("rql_phase1", "00007")]);
        let official = json!({"sqlite_scope_policy": {
            "schema_version": POLICY_SCHEMA, "path": "x", "sha256": policy.sha256}});
        policy.check_recorded(&official).expect("same file");
        let mut stale = official.clone();
        stale["sqlite_scope_policy"]["sha256"] = json!("0".repeat(64));
        assert!(policy.check_recorded(&stale).is_err());
        assert!(policy.check_recorded(&json!({})).is_err());
    }

    #[test]
    fn raw_skips_must_name_their_exception() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("raw.jsonl");
        let write = |text: &str| fs::write(&path, text).expect("raw");
        write(concat!(
            "{\"case_id\":\"00007\",\"status\":\"skipped\",\"policy_exception_id\":\"rql_phase1:00007\"}\n",
            "{\"case_id\":\"00008\",\"status\":\"passed\"}\n",
        ));
        assert_eq!(
            raw_skipped_case_ids(&path, "rql_phase1").expect("raw"),
            ids(&["00007"])
        );
        assert!(raw_skipped_case_ids(&path, "memory").is_err());
        write("{\"case_id\":\"00007\",\"status\":\"skipped\",\"policy_exception_id\":null}\n");
        assert!(raw_skipped_case_ids(&path, "rql_phase1").is_err());
    }

    #[test]
    fn committed_policy_is_well_formed() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let policy = ScopePolicy::load(&repo_root).expect("committed policy");
        assert!(policy.by_suite["sqlite_parity"].is_empty());
        assert!(policy.by_suite["memory"].is_empty());
        assert!(!policy.by_suite["rql_phase1"].is_empty());
    }

    #[test]
    fn malformed_policies_are_rejected() {
        for exceptions in [
            json!([{"suite": "beyond_sqlite", "case_id": "1", "name": "N", "kind": "k", "reason": "r", "owner": "o", "expiry": "e"}]),
            json!([{"suite": "memory", "case_id": "00001", "name": "N", "kind": "k", "reason": "", "owner": "o", "expiry": "e"}]),
            json!([
                {"suite": "memory", "case_id": "00001", "name": "N", "kind": "k", "reason": "r", "owner": "o", "expiry": "e"},
                {"suite": "memory", "case_id": "00001", "name": "N", "kind": "k", "reason": "r", "owner": "o", "expiry": "e"}
            ]),
        ] {
            let text = json!({"schema_version": POLICY_SCHEMA, "exceptions": exceptions});
            assert!(
                ScopePolicy::parse(text.to_string().as_bytes()).is_err(),
                "{text}"
            );
        }
        assert!(ScopePolicy::parse(br#"{"schema_version":"v0","exceptions":[]}"#).is_err());
    }
}
