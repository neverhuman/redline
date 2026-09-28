//! The PostgreSQL gate's qualification report (`postgres-qualification.json`),
//! held to the beyond_sqlite suite summary and the artifacts it names.
use std::collections::BTreeSet;

use anyhow::{Result, bail};
use serde_json::Value;

/// v2 splits `passed` into row matches and expected rejections, and
/// `failed` into declared-unsupported refusals and mismatches.
pub(crate) const SCHEMA: &str = "redline-postgres-qualification-v2";

/// What an agreeing case establishes; the report must say exactly this.
const COMPARISON: &str = "normalized SQL-shell transcript agreement";

/// The hashes the report must name.
pub(crate) struct Hashes<'a> {
    pub(crate) raw: &'a str,
    pub(crate) provenance: &'a str,
    pub(crate) policy: &'a str,
}

pub(crate) fn check(pg: &Value, summary: &Value, hashes: &Hashes<'_>) -> Result<()> {
    if pg["schema_version"] != SCHEMA
        || pg["comparison"] != COMPARISON
        || pg["regression"] != "passed"
        || pg["required"] != summary["total"]
        || pg["passed"] != summary["passed"]
        || pg["failed"] != summary["failed"]
        || pg["skipped"] != 0
        || pg["unverified"] != 0
        || pg["raw_sha256"] != hashes.raw
        || pg["provenance_sha256"] != hashes.provenance
        || pg["policy_sha256"] != hashes.policy
    {
        bail!("PostgreSQL regression proof is missing, inconsistent, or failed");
    }
    let ids = |field: &str| -> Result<BTreeSet<String>> {
        let Some(list) = pg[field].as_array() else {
            bail!("PostgreSQL qualification lacks {field}");
        };
        list.iter()
            .map(|id| {
                id.as_str().map(str::to_owned).ok_or_else(|| {
                    anyhow::anyhow!("PostgreSQL qualification {field} holds a non-id")
                })
            })
            .collect()
    };
    let rejections = ids("expected_rejections")?;
    let unsupported = ids("declared_unsupported")?;
    let mismatches = ids("mismatches")?;
    let failed_cases = ids("failed_cases")?;
    let count = |value: &Value| value.as_u64().unwrap_or(u64::MAX);
    let positive = count(&pg["positive_matches"]);
    if positive.checked_add(rejections.len() as u64) != Some(count(&pg["passed"]))
        || !unsupported.is_disjoint(&mismatches)
        || unsupported
            .union(&mismatches)
            .cloned()
            .collect::<BTreeSet<_>>()
            != failed_cases
        || failed_cases.len() as u64 != count(&pg["failed"])
    {
        bail!(
            "PostgreSQL qualification split does not add up: {positive} row matches + {} expected rejections vs {} passed; {} declared unsupported + {} mismatches vs {} failed",
            rejections.len(),
            pg["passed"],
            unsupported.len(),
            mismatches.len(),
            pg["failed"]
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> Value {
        serde_json::json!({"total": 265, "passed": 264, "failed": 1})
    }

    fn hashes() -> Hashes<'static> {
        Hashes {
            raw: "raw",
            provenance: "prov",
            policy: "policy",
        }
    }

    /// A v2 report for 252 row matches, 12 expected rejections and one
    /// declared-unsupported refusal.
    fn report() -> Value {
        let rejections: Vec<String> = (0..12).map(|i| format!("R{i}")).collect();
        serde_json::json!({
            "schema_version": "redline-postgres-qualification-v2",
            "comparison": "normalized SQL-shell transcript agreement",
            "qualification": "failed",
            "regression": "passed",
            "required": 265,
            "passed": 264,
            "failed": 1,
            "skipped": 0,
            "unverified": 0,
            "positive_matches": 252,
            "expected_rejections": rejections,
            "declared_unsupported": ["U"],
            "mismatches": [],
            "failed_cases": ["U"],
            "raw_sha256": "raw",
            "provenance_sha256": "prov",
            "policy_sha256": "policy",
        })
    }

    #[test]
    fn a_v2_report_with_a_consistent_split_is_accepted() {
        check(&report(), &summary(), &hashes()).unwrap();
    }

    #[test]
    fn a_report_whose_split_does_not_add_up_is_refused() {
        for (field, value) in [
            (
                "schema_version",
                serde_json::json!("redline-postgres-qualification-v1"),
            ),
            ("positive_matches", serde_json::json!(253)),
            ("expected_rejections", serde_json::json!(["R0"])),
            ("declared_unsupported", serde_json::json!([])),
            ("mismatches", serde_json::json!(["M"])),
            ("failed_cases", serde_json::json!(["M"])),
            ("comparison", serde_json::json!("typed parity")),
            ("regression", serde_json::json!("failed")),
            ("raw_sha256", serde_json::json!("other")),
        ] {
            let mut pg = report();
            pg[field] = value;
            assert!(check(&pg, &summary(), &hashes()).is_err(), "{field}");
        }
    }
}
