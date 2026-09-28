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
mod tests {
    use super::*;

    fn baseline(entries: &[(&str, &str)]) -> Baseline {
        let failures = entries
            .iter()
            .map(|(suite, case_id)| {
                json!({"suite": suite, "case_id": case_id, "name": "N",
                       "stage": "target_semantic_failure", "reason": "r", "owner": "o"})
            })
            .collect::<Vec<_>>();
        let text =
            json!({"schema_version": BASELINE_SCHEMA, "description": "d", "failures": failures});
        Baseline::parse(text.to_string().as_bytes()).expect("baseline")
    }

    fn entry(failed: &[&str], listed: &[&str]) -> Value {
        json!({"failed": failed.len(), "failed_case_ids": failed, "known_failure_ids": listed})
    }

    fn ids(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    /// Raw failures, each failing with the given verdict_reasons.
    fn failures(cases: &[(&str, &[&str])]) -> RawFailures {
        cases
            .iter()
            .map(|(case_id, reasons)| ((*case_id).to_owned(), ids(reasons)))
            .collect()
    }

    /// Raw failures, each a target semantic failure (the fixture stage).
    fn failed(case_ids: &[&str]) -> RawFailures {
        case_ids
            .iter()
            .map(|case_id| ((*case_id).to_owned(), ids(&["target_semantic_failure"])))
            .collect()
    }

    #[test]
    fn sqlite_failures_must_equal_the_known_failures_baseline() {
        let known = baseline(&[("sqlite_parity", "10547"), ("memory", "10547")]);
        let raw = failed(&["10547"]);
        known
            .check_suite("sqlite_parity", &entry(&["10547"], &["10547"]), &raw)
            .expect("listed failure");
        known
            .check_suite("memory", &entry(&["10547"], &["10547"]), &raw)
            .expect("listed failure");
        // A failure the baseline does not list.
        let raw = failed(&["10547", "00076"]);
        let error = known
            .check_suite(
                "sqlite_parity",
                &entry(&["00076", "10547"], &["10547"]),
                &raw,
            )
            .expect_err("unlisted failure");
        assert!(
            format!("{error:#}").contains("failed but not listed [\"00076\"]"),
            "{error:#}"
        );
        // A listed case that passed.
        let error = known
            .check_suite("sqlite_parity", &entry(&[], &["10547"]), &failed(&[]))
            .expect_err("listed pass");
        assert!(
            format!("{error:#}").contains("remove those from the baseline"),
            "{error:#}"
        );
    }

    #[test]
    fn declared_failures_must_match_the_raw_records() {
        let known = baseline(&[("sqlite_parity", "10547")]);
        // The summary hides a failure the raw records show.
        let error = known
            .check_suite(
                "sqlite_parity",
                &entry(&[], &["10547"]),
                &failed(&["10547"]),
            )
            .expect_err("hidden failure");
        assert!(
            format!("{error:#}").contains("raw records fail 1"),
            "{error:#}"
        );
        // The run was gated with a different list than the committed one.
        let error = known
            .check_suite(
                "sqlite_parity",
                &entry(&["10547"], &[]),
                &failed(&["10547"]),
            )
            .expect_err("different baseline");
        assert!(
            format!("{error:#}").contains("was gated with known failures"),
            "{error:#}"
        );
        // No declared lists at all.
        assert!(
            known
                .check_suite("sqlite_parity", &json!({"failed": 1}), &failed(&["10547"]))
                .is_err()
        );
    }

    #[test]
    fn the_run_must_record_the_committed_baseline() {
        let known = baseline(&[("memory", "00134")]);
        let official = json!({"sqlite_known_failures": {
            "schema_version": BASELINE_SCHEMA, "path": "x", "sha256": known.sha256}});
        known.check_recorded(&official).expect("same file");
        let mut stale = official.clone();
        stale["sqlite_known_failures"]["sha256"] = json!("0".repeat(64));
        assert!(known.check_recorded(&stale).is_err());
        assert!(known.check_recorded(&json!({})).is_err());
    }

    #[test]
    fn malformed_baselines_are_rejected() {
        for failures in [
            json!([{"suite": "rql_phase1", "case_id": "1", "name": "N", "stage": "s", "reason": "r", "owner": "o"}]),
            json!([{"suite": "memory", "case_id": "1", "name": "N", "stage": "s", "reason": "", "owner": "o"}]),
            json!([
                {"suite": "memory", "case_id": "1", "name": "N", "stage": "differential_mismatch", "reason": "r", "owner": "o"},
                {"suite": "memory", "case_id": "1", "name": "N", "stage": "differential_mismatch", "reason": "r", "owner": "o"}
            ]),
            // A reference contract failure is a corpus defect, never a
            // baseline entry.
            json!([{"suite": "memory", "case_id": "1", "name": "N", "stage": "reference_contract_failure", "reason": "r", "owner": "o"}]),
        ] {
            let text = json!({"schema_version": BASELINE_SCHEMA, "failures": failures});
            assert!(
                Baseline::parse(text.to_string().as_bytes()).is_err(),
                "{text}"
            );
        }
        assert!(Baseline::parse(br#"{"schema_version":"v0","failures":[]}"#).is_err());
    }

    #[test]
    fn committed_baseline_is_well_formed() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let known = Baseline::load(&repo_root).expect("committed baseline");
        // sqlite_parity and memory run the same corpus through the same
        // comparator, so they fail the same cases.
        assert_eq!(known.by_suite["sqlite_parity"], known.by_suite["memory"]);
    }

    #[test]
    fn a_listed_failure_must_fail_at_its_listed_stage() {
        // 10547 is listed as target_semantic_failure. A run that fails it
        // another way was rejected by the runner's gate, but its evidence
        // was written first, so the processor must reject it too.
        let known = baseline(&[("sqlite_parity", "10547")]);
        let entry = entry(&["10547"], &["10547"]);
        known
            .check_suite(
                "sqlite_parity",
                &entry,
                &failures(&[("10547", &["target_semantic_failure"])]),
            )
            .expect("listed stage");
        for reasons in [
            &["reference_contract_failure"][..],
            &["differential_mismatch"],
            &["target_semantic_failure", "execution_failure"],
            &["unrecorded"],
        ] {
            let error = known
                .check_suite("sqlite_parity", &entry, &failures(&[("10547", reasons)]))
                .expect_err("another stage");
            assert!(
                format!("{error:#}").contains(
                    "suite sqlite_parity case 10547 is listed as target_semantic_failure, but its raw records fail as"
                ),
                "{reasons:?}: {error:#}"
            );
        }
    }

    #[test]
    fn raw_failures_come_from_failed_records() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("raw.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"case_id\":\"00001\",\"status\":\"passed\",\"verdict_reason\":\"passed\"}\n",
                "{\"case_id\":\"00002\",\"status\":\"failed\",\"verdict_reason\":\"differential_mismatch\"}\n",
                "{\"case_id\":\"00002\",\"status\":\"passed\",\"verdict_reason\":\"passed\"}\n",
                "{\"case_id\":\"00003\",\"status\":\"skipped\",\"verdict_reason\":\"skipped\"}\n",
                "{\"case_id\":\"00004\",\"status\":\"failed\",\"verdict_reason\":\"target_semantic_failure\"}\n",
                "{\"case_id\":\"00004\",\"status\":\"failed\",\"verdict_reason\":\"execution_failure\"}\n",
                "{\"case_id\":\"00005\",\"status\":\"failed\"}\n",
            ),
        )
        .expect("write raw");
        assert_eq!(
            raw_failures(&path).expect("raw"),
            failures(&[
                ("00002", &["differential_mismatch"]),
                ("00004", &["execution_failure", "target_semantic_failure"]),
                ("00005", &["unrecorded"]),
            ])
        );
    }
}
