//! The reviewed PostgreSQL regression policy
//! (`metadata/beyond_sqlite/postgres-regression.json`) and its two-way
//! ratchet.
use std::collections::BTreeSet;

use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::Value;

use super::cases::Outcomes;

pub(super) const SCHEMA: &str = "redline-postgres-regression-v2";

/// v2 adds `declared_rejections` and `declared_unsupported`, and refuses an
/// unknown field, so a misspelt list cannot silently mean "none".
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Baseline {
    schema_version: String,
    /// The commit the policy was last reviewed against; informational.
    #[serde(default)]
    #[allow(dead_code)]
    previous_source_commit: Option<String>,
    /// Why the lists are what they are. It carries no count: the counts are
    /// the lists themselves.
    #[allow(dead_code)]
    reason: String,
    corpus_sha256: String,
    reference_settings: String,
    image_digest: String,
    server_binary_sha256: String,
    /// Cases allowed to mismatch.
    failed_cases: BTreeSet<String>,
    /// Cases allowed to fail with an `unsupported capability:` refusal.
    #[serde(default)]
    declared_unsupported: BTreeSet<String>,
    /// The declared rejections the policy was reviewed against. When
    /// present it must equal the set the corpus declares, so prose and
    /// policy cannot drift from the corpus.
    #[serde(default)]
    declared_rejections: Option<BTreeSet<String>>,
}

/// The ratchet's verdict.
#[derive(Debug)]
pub(super) struct Ratchet {
    /// No mismatch outside `failed_cases`, and no unsupported refusal
    /// outside `declared_unsupported`.
    pub(super) passed: bool,
    /// Cases the policy lists as failing or unsupported that now agree.
    pub(super) newly_passing: BTreeSet<String>,
}

/// Checks the policy against the corpus and the run's reference, then
/// ratchets the run's failures against it in both directions.
pub(super) fn ratchet(
    policy: &Baseline,
    split: &Outcomes,
    required: &BTreeSet<String>,
    corpus_declared_rejections: &BTreeSet<String>,
    corpus_sha256: &str,
    reference: &Value,
) -> Result<Ratchet> {
    ensure!(
        policy.schema_version == SCHEMA,
        "unknown regression policy version {:?}; expected {SCHEMA}",
        policy.schema_version
    );
    ensure!(
        policy.corpus_sha256 == corpus_sha256,
        "regression policy uses a different corpus"
    );
    ensure!(
        policy.failed_cases.is_subset(required) && policy.declared_unsupported.is_subset(required),
        "regression policy contains unknown cases"
    );
    ensure!(
        policy
            .failed_cases
            .is_disjoint(&policy.declared_unsupported),
        "regression policy lists a case as both failed and declared unsupported"
    );
    if let Some(declared) = &policy.declared_rejections {
        ensure!(
            declared == corpus_declared_rejections,
            "regression policy declares rejections {:?}, but the corpus declares {:?}",
            declared,
            corpus_declared_rejections
        );
    }
    ensure!(
        reference["settings"] == policy.reference_settings,
        "reference settings changed"
    );
    ensure!(
        reference["image_digest"] == policy.image_digest
            || reference["server_binary_sha256"] == policy.server_binary_sha256,
        "unknown reference digest"
    );
    // The ratchet has two directions. The subset checks catch a failure set
    // that grows. A case the policy lists that has since started agreeing is
    // caught by `newly_passing` -- otherwise a closed gap stays recorded as
    // broken, which is how metadata/beyond_sqlite/skip-list.toml came to mark
    // 29 cases "deferred" while every one of them passed.
    let listed: BTreeSet<String> = policy
        .failed_cases
        .union(&policy.declared_unsupported)
        .cloned()
        .collect();
    Ok(Ratchet {
        passed: split.mismatches.is_subset(&policy.failed_cases)
            && split
                .declared_unsupported
                .is_subset(&policy.declared_unsupported),
        newly_passing: listed.difference(&split.failed()).cloned().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|id| (*id).to_owned()).collect()
    }

    fn policy(failed: &[&str], unsupported: &[&str], declared: Option<&[&str]>) -> Baseline {
        let mut value = serde_json::json!({
            "schema_version": SCHEMA,
            "reason": "reviewed",
            "corpus_sha256": "corpus",
            "reference_settings": "160015|C|C|UTC",
            "image_digest": "sha256:pin",
            "server_binary_sha256": "server",
            "failed_cases": failed,
            "declared_unsupported": unsupported,
        });
        if let Some(declared) = declared {
            value["declared_rejections"] = serde_json::json!(declared);
        }
        serde_json::from_value(value).unwrap()
    }

    fn split(mismatches: &[&str], unsupported: &[&str]) -> Outcomes {
        Outcomes {
            mismatches: ids(mismatches),
            declared_unsupported: ids(unsupported),
            ..Outcomes::default()
        }
    }

    fn run(policy: &Baseline, split: &Outcomes) -> Result<Ratchet> {
        let reference = serde_json::json!({
            "settings": "160015|C|C|UTC", "image_digest": "sha256:pin",
        });
        ratchet(
            policy,
            split,
            &ids(&["a", "b", "r"]),
            &ids(&["r"]),
            "corpus",
            &reference,
        )
    }

    #[test]
    fn declared_rejections_must_equal_the_corpus() {
        assert!(run(&policy(&[], &[], Some(&["r"])), &split(&[], &[])).is_ok());
        assert!(run(&policy(&[], &[], None), &split(&[], &[])).is_ok());
        for wrong in [&["r", "a"][..], &[][..], &["a"][..]] {
            let err = run(&policy(&[], &[], Some(wrong)), &split(&[], &[]))
                .unwrap_err()
                .to_string();
            assert!(err.contains("but the corpus declares"), "{err}");
        }
    }

    #[test]
    fn an_unknown_policy_field_is_refused() {
        let value = serde_json::json!({
            "schema_version": SCHEMA, "reason": "r", "corpus_sha256": "c",
            "reference_settings": "s", "image_digest": "i", "server_binary_sha256": "b",
            "failed_cases": [], "declared_unsuported": ["a"],
        });
        assert!(serde_json::from_value::<Baseline>(value).is_err());
    }

    #[test]
    fn declared_unsupported_ratchets_both_ways() {
        // Listed and observed: the gate passes.
        let verdict = run(&policy(&[], &["a"], None), &split(&[], &["a"])).unwrap();
        assert!(verdict.passed && verdict.newly_passing.is_empty());
        // Observed but not listed, or listed only as a mismatch: it fails.
        assert!(
            !run(&policy(&[], &[], None), &split(&[], &["a"]))
                .unwrap()
                .passed
        );
        assert!(
            !run(&policy(&["a"], &[], None), &split(&[], &["a"]))
                .unwrap()
                .passed
        );
        // Listed but now agreeing: named as newly passing.
        let verdict = run(&policy(&[], &["a"], None), &split(&[], &[])).unwrap();
        assert_eq!(verdict.newly_passing, ids(&["a"]));
        // A listed unsupported case that now mismatches is not allowed.
        assert!(
            !run(&policy(&[], &["a"], None), &split(&["a"], &[]))
                .unwrap()
                .passed
        );
        // One case cannot be in both lists.
        assert!(run(&policy(&["a"], &["a"], None), &split(&[], &[])).is_err());
    }
}
