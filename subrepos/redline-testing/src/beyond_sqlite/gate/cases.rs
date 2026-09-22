//! Completeness and execution checks over PostgreSQL JSONL case outcomes.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::collections::BTreeSet;

/// `declared_rejections` are the cases whose corpus entry states the message
/// the target must produce when both engines reject the script. Only those may
/// pass with a non-zero exit; for every other case a negative result that
/// "matches" proves nothing, because two engines can be unhappy for unrelated
/// reasons. The gate re-derives this from the corpus rather than trusting the
/// runner, so a runner that started passing negatives on its own would still
/// be caught here.
pub(super) fn outcomes(
    raw: &str,
    required: &BTreeSet<String>,
    declared_rejections: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let mut oracle = BTreeSet::new();
    let mut target = BTreeSet::new();
    let mut failed = BTreeSet::new();
    for line in raw.lines() {
        let row: Value = serde_json::from_str(line)?;
        let profile = row["profile"].as_str().unwrap_or_default();
        if profile == "beyond_sqlite" {
            continue;
        } // Historical feature taxonomy, not executions.
        ensure!(
            matches!(profile, "beyond_sqlite_oracle" | "beyond_sqlite_target"),
            "unknown Postgres profile"
        );
        let id = row["case_id"]
            .as_str()
            .context("case id missing")?
            .to_owned();
        ensure!(required.contains(&id), "unknown Postgres case {id}");
        let seen = if profile == "beyond_sqlite_oracle" {
            &mut oracle
        } else {
            &mut target
        };
        ensure!(seen.insert(id.clone()), "duplicate {profile} case {id}");
        let status = row["status"].as_str().context("case status missing")?;
        if profile == "beyond_sqlite_oracle" {
            ensure!(
                status == "passed",
                "reference case {id} failed or was skipped"
            );
        } else {
            ensure!(
                matches!(status, "passed" | "failed"),
                "target case {id} was not executed"
            );
            ensure!(
                row["target_exit_code"]
                    .as_i64()
                    .is_some_and(|code| code >= 0)
                    && row["reference_exit_code"]
                        .as_i64()
                        .is_some_and(|code| code >= 0),
                "case {id} lacks process outcomes"
            );
            if status == "failed" {
                failed.insert(id);
            } else {
                ensure!(
                    (row["reference_exit_code"] == 0 && row["target_exit_code"] == 0)
                        || declared_rejections.contains(&id),
                    "case {id} needs semantic error assertions before a negative result can pass"
                );
            }
        }
    }
    ensure!(
        !required.is_empty() && oracle == *required && target == *required,
        "missing Postgres comparisons: required={}, oracle={}, target={}",
        required.len(),
        oracle.len(),
        target.len()
    );
    Ok(failed)
}

#[cfg(test)]
pub(super) fn rows(status: &str) -> String {
    format!(
        "{{\"profile\":\"beyond_sqlite_oracle\",\"case_id\":\"c\",\"status\":\"passed\"}}\n{{\"profile\":\"beyond_sqlite_target\",\"case_id\":\"c\",\"status\":\"{status}\",\"target_exit_code\":0,\"reference_exit_code\":0}}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_duplicate_skipped_unknown_and_empty_runs_fail() {
        let required = BTreeSet::from(["c".to_owned()]);
        let none = BTreeSet::new();
        let good = rows("passed");
        assert!(outcomes(&good, &required, &none).unwrap().is_empty());
        for bad in [
            String::new(),
            good.lines().next().unwrap().into(),
            good.repeat(2),
            rows("skipped"),
            good.replace("\"c\"", "\"unknown\""),
        ] {
            assert!(outcomes(&bad, &required, &none).is_err(), "{bad}");
        }
        assert!(outcomes("", &BTreeSet::new(), &none).is_err());
        assert_eq!(
            outcomes(&rows("failed"), &required, &none).unwrap(),
            required
        );
    }

    #[test]
    fn a_negative_result_passes_only_where_the_corpus_declared_the_message() {
        let required = BTreeSet::from(["c".to_owned()]);
        let rejecting = rows("passed")
            .replace("\"target_exit_code\":0", "\"target_exit_code\":3")
            .replace("\"reference_exit_code\":0", "\"reference_exit_code\":3");
        // Undeclared: a matching non-zero exit is not evidence of anything.
        let err = outcomes(&rejecting, &required, &BTreeSet::new())
            .unwrap_err()
            .to_string();
        assert!(err.contains("needs semantic error assertions"), "{err}");
        // Declared: the corpus stated what the target must say, so the runner
        // is allowed to have checked it.
        let declared = BTreeSet::from(["c".to_owned()]);
        assert!(
            outcomes(&rejecting, &required, &declared)
                .unwrap()
                .is_empty()
        );
    }
}
