//! Completeness and execution checks over PostgreSQL JSONL case outcomes.
//!
//! The gate does not take a target row's `status: passed` on trust (PG-06).
//! It re-derives each case's contract from the corpus -- the exit PostgreSQL
//! must give, and for a declared rejection the text the target must print
//! and whether the case has setup -- and re-checks the pass against the
//! assertion evidence the runner recorded (`beyond_sqlite::assertion`).
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::super::case::BeyondCase;
use super::super::transcript::COMPARATOR_VERSION;

/// What the corpus says a case must do.
#[derive(Debug, Clone)]
pub(super) struct CaseContract {
    /// The exit PostgreSQL gives: 0, or 3 for a reviewed negative script.
    pub(super) expected_reference_exit: i32,
    /// For a declared rejection, the text the target's stderr must contain.
    pub(super) declared_error: Option<String>,
    /// Whether the case runs setup before the statement under test.
    pub(super) has_setup: bool,
}

/// Every corpus case's contract, keyed `BEYOND-CASE-NNNNN`.
pub(super) fn contracts(cases: &[BeyondCase]) -> BTreeMap<String, CaseContract> {
    cases
        .iter()
        .map(|case| {
            (
                format!("BEYOND-CASE-{:05}", case.id),
                CaseContract {
                    expected_reference_exit: case.expected_reference_exit,
                    declared_error: case.expected_target_stderr_contains.clone(),
                    has_setup: case.setup_stdin.is_some(),
                },
            )
        })
        .collect()
}

/// The cases split by how they ended.
#[derive(Debug, Default)]
pub(super) struct Outcomes {
    /// Both engines exited 0 and printed the same normalized rows.
    pub(super) positive_matches: BTreeSet<String>,
    /// Both engines rejected a declared case, the target printed the
    /// declared text, and its setup ran on its own.
    pub(super) expected_rejections: BTreeSet<String>,
    /// Every case that did not agree.
    pub(super) failed: BTreeSet<String>,
}

/// `contracts` names every required case. Only a declared rejection may pass
/// with a non-zero exit; for every other case a negative result that
/// "matches" proves nothing, because two engines can be unhappy for
/// unrelated reasons. A row that claims a pass its evidence does not support
/// is an error, not a failure: the runner that wrote it cannot be trusted.
pub(super) fn outcomes(raw: &str, contracts: &BTreeMap<String, CaseContract>) -> Result<Outcomes> {
    let mut oracle = BTreeSet::new();
    let mut target = BTreeSet::new();
    let mut split = Outcomes::default();
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
        let contract = contracts
            .get(&id)
            .with_context(|| format!("unknown Postgres case {id}"))?;
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
            continue;
        }
        ensure!(
            matches!(status, "passed" | "failed"),
            "target case {id} was not executed"
        );
        let exit = |field: &str| row[field].as_i64().filter(|code| *code >= 0);
        let (Some(reference_exit), Some(target_exit)) =
            (exit("reference_exit_code"), exit("target_exit_code"))
        else {
            bail!("case {id} lacks process outcomes");
        };
        ensure!(
            row["comparator_version"] == COMPARATOR_VERSION,
            "case {id} was compared by {}, not {COMPARATOR_VERSION}",
            row["comparator_version"]
        );
        ensure!(
            row["expected_reference_exit"].as_i64()
                == Some(i64::from(contract.expected_reference_exit)),
            "case {id} records expected reference exit {}; the corpus says {}",
            row["expected_reference_exit"],
            contract.expected_reference_exit
        );
        if status == "failed" {
            split.failed.insert(id);
            continue;
        }
        if recheck_pass(&id, &row, contract, (reference_exit, target_exit))? {
            split.expected_rejections.insert(id);
        } else {
            split.positive_matches.insert(id);
        }
    }
    let required: BTreeSet<String> = contracts.keys().cloned().collect();
    ensure!(
        !required.is_empty() && oracle == required && target == required,
        "missing Postgres comparisons: required={}, oracle={}, target={}",
        required.len(),
        oracle.len(),
        target.len()
    );
    Ok(split)
}

/// Re-checks a row the runner marked passed; true for an expected rejection,
/// false for a positive match.
fn recheck_pass(
    id: &str,
    row: &Value,
    contract: &CaseContract,
    (reference_exit, target_exit): (i64, i64),
) -> Result<bool> {
    let Some(declared) = contract.declared_error.as_deref() else {
        ensure!(
            reference_exit == 0 && target_exit == 0,
            "case {id} needs semantic error assertions before a negative result can pass"
        );
        ensure!(
            contract.expected_reference_exit == 0,
            "case {id} expects PostgreSQL to exit {} but declares no error text",
            contract.expected_reference_exit
        );
        check_stdout(id, row)?;
        return Ok(false);
    };
    let expected = i64::from(contract.expected_reference_exit);
    ensure!(
        expected != 0 && reference_exit == expected && target_exit == reference_exit,
        "case {id} passed with exits (reference {reference_exit}, target {target_exit}); \
         the corpus expects both to exit {expected}"
    );
    check_stdout(id, row)?;
    let stderr = row["target_stderr"].as_str();
    ensure!(
        row["target_stderr_truncated"] == false
            && stderr.is_some_and(|text| {
                text.contains(declared)
                    && row["target_stderr_sha256"].as_str() == Some(sha256(text).as_str())
            })
            && row["declared_error_matched"] == true,
        "case {id} passed without the declared error text {declared:?} in its recorded stderr"
    );
    if contract.has_setup {
        ensure!(
            row["setup_exit_code"] == 0,
            "case {id} passed without evidence that its setup ran on its own \
             (setup_exit_code {})",
            row["setup_exit_code"]
        );
    }
    Ok(true)
}

/// Both complete normalized stdouts were recorded and hash equal.
fn check_stdout(id: &str, row: &Value) -> Result<()> {
    let digest = |field: &str| {
        row[field]
            .as_str()
            .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
    };
    let reference = digest("reference_normalized_stdout_sha256");
    ensure!(
        reference.is_some() && reference == digest("target_normalized_stdout_sha256"),
        "case {id} passed without equal normalized stdout hashes"
    );
    Ok(())
}

fn sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// A complete, agreeing pair of rows for case `id` under `contract`, as a
/// correct runner writes them.
#[cfg(test)]
pub(super) fn agreeing_rows(id: &str, contract: &CaseContract) -> String {
    let exit = contract.expected_reference_exit;
    let stderr = contract
        .declared_error
        .as_deref()
        .map(|text| format!("Error: 1002: {text}\n"))
        .unwrap_or_default();
    let stdout = sha256("1\n");
    let oracle = serde_json::json!({
        "profile": "beyond_sqlite_oracle", "case_id": id, "status": "passed",
    });
    let target = serde_json::json!({
        "profile": "beyond_sqlite_target",
        "case_id": id,
        "status": "passed",
        "reference_exit_code": exit,
        "target_exit_code": exit,
        "expected_reference_exit": exit,
        "setup_exit_code": if contract.declared_error.is_some() && contract.has_setup { Some(0) } else { None },
        "declared_error_matched": contract.declared_error.as_ref().map(|_| true),
        "reference_normalized_stdout_sha256": stdout,
        "target_normalized_stdout_sha256": stdout,
        "target_stderr": stderr,
        "target_stderr_truncated": false,
        "target_stderr_sha256": sha256(&stderr),
        "comparator_version": COMPARATOR_VERSION,
    });
    format!("{oracle}\n{target}\n")
}

/// An agreeing run over the whole corpus.
#[cfg(test)]
pub(super) fn agreeing_run(contracts: &BTreeMap<String, CaseContract>) -> String {
    contracts
        .iter()
        .map(|(id, contract)| agreeing_rows(id, contract))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn positive() -> BTreeMap<String, CaseContract> {
        BTreeMap::from([(
            "c".to_owned(),
            CaseContract {
                expected_reference_exit: 0,
                declared_error: None,
                has_setup: false,
            },
        )])
    }

    fn declared(has_setup: bool) -> BTreeMap<String, CaseContract> {
        BTreeMap::from([(
            "c".to_owned(),
            CaseContract {
                expected_reference_exit: 3,
                declared_error: Some("invalid input value for enum".to_owned()),
                has_setup,
            },
        )])
    }

    /// `agreeing_rows` for case `c`, with one target field replaced.
    fn with(contracts: &BTreeMap<String, CaseContract>, edits: &[(&str, Value)]) -> String {
        let rows = agreeing_rows("c", &contracts["c"]);
        let (oracle, target) = rows.trim_end().split_once('\n').unwrap();
        let mut target: Value = serde_json::from_str(target).unwrap();
        for (field, value) in edits {
            target[*field] = value.clone();
        }
        format!("{oracle}\n{target}\n")
    }

    fn error(contracts: &BTreeMap<String, CaseContract>, raw: &str) -> String {
        outcomes(raw, contracts).unwrap_err().to_string()
    }

    #[test]
    fn missing_duplicate_skipped_unknown_and_empty_runs_fail() {
        let contracts = positive();
        let good = agreeing_rows("c", &contracts["c"]);
        let split = outcomes(&good, &contracts).unwrap();
        assert!(split.failed.is_empty());
        assert_eq!(split.positive_matches, BTreeSet::from(["c".to_owned()]));
        for bad in [
            String::new(),
            good.lines().next().unwrap().into(),
            good.repeat(2),
            with(&contracts, &[("status", "skipped".into())]),
            good.replace("\"c\"", "\"unknown\""),
        ] {
            assert!(outcomes(&bad, &contracts).is_err(), "{bad}");
        }
        assert!(outcomes("", &BTreeMap::new()).is_err());
        let failed = with(&contracts, &[("status", "failed".into())]);
        assert_eq!(
            outcomes(&failed, &contracts).unwrap().failed,
            BTreeSet::from(["c".to_owned()])
        );
    }

    #[test]
    fn a_negative_result_passes_only_where_the_corpus_declared_the_message() {
        let rejecting = [
            ("reference_exit_code", Value::from(3)),
            ("target_exit_code", Value::from(3)),
        ];
        // Undeclared: a matching non-zero exit is not evidence of anything.
        let err = error(&positive(), &with(&positive(), &rejecting));
        assert!(err.contains("needs semantic error assertions"), "{err}");
        // Declared, with the evidence a correct runner records.
        let split = outcomes(&agreeing_rows("c", &declared(true)["c"]), &declared(true)).unwrap();
        assert_eq!(split.expected_rejections, BTreeSet::from(["c".to_owned()]));
        assert!(split.positive_matches.is_empty());
    }

    #[test]
    fn declared_rejection_with_mismatched_exits_fails() {
        let contracts = declared(false);
        for (reference, target) in [(0, 3), (3, 0), (7, 7)] {
            let raw = with(
                &contracts,
                &[
                    ("reference_exit_code", reference.into()),
                    ("target_exit_code", target.into()),
                ],
            );
            let err = error(&contracts, &raw);
            assert!(
                err.contains("the corpus expects both to exit 3"),
                "({reference},{target}): {err}"
            );
        }
    }

    #[test]
    fn wrong_error_text_fails() {
        let contracts = declared(false);
        let other = "Error: 1002: unsupported sql: DROP TYPE\n";
        for edits in [
            // The runner's own flag says it matched; the stderr says not.
            vec![
                ("target_stderr", Value::from(other)),
                ("target_stderr_sha256", sha256(other).into()),
            ],
            // The text is there but the recorded hash is not of it.
            vec![("target_stderr_sha256", sha256(other).into())],
            // The text was cut, so it is not the whole stderr.
            vec![("target_stderr_truncated", true.into())],
            // The runner says it did not match.
            vec![("declared_error_matched", false.into())],
            vec![("target_stderr", Value::Null)],
        ] {
            let err = error(&contracts, &with(&contracts, &edits));
            assert!(err.contains("without the declared error text"), "{err}");
        }
    }

    #[test]
    fn missing_setup_evidence_fails() {
        let contracts = declared(true);
        for setup in [Value::Null, Value::from(1), Value::from(-1)] {
            let err = error(&contracts, &with(&contracts, &[("setup_exit_code", setup)]));
            assert!(err.contains("setup ran on its own"), "{err}");
        }
        // A case with no setup needs no setup evidence.
        let no_setup = declared(false);
        assert!(outcomes(&agreeing_rows("c", &no_setup["c"]), &no_setup).is_ok());
    }

    #[test]
    fn unequal_stdout_hashes_fail() {
        for contracts in [positive(), declared(false)] {
            for edits in [
                vec![("target_normalized_stdout_sha256", sha256("0\n").into())],
                vec![("reference_normalized_stdout_sha256", Value::Null)],
                vec![
                    ("reference_normalized_stdout_sha256", "aa".into()),
                    ("target_normalized_stdout_sha256", "aa".into()),
                ],
            ] {
                let err = error(&contracts, &with(&contracts, &edits));
                assert!(err.contains("equal normalized stdout hashes"), "{err}");
            }
        }
    }

    #[test]
    fn records_from_another_comparator_or_contract_fail() {
        let contracts = positive();
        let err = error(
            &contracts,
            &with(&contracts, &[("comparator_version", Value::Null)]),
        );
        assert!(
            err.contains("not redline-beyond-sqlite-comparator-v2"),
            "{err}"
        );
        let err = error(
            &contracts,
            &with(&contracts, &[("expected_reference_exit", 3.into())]),
        );
        assert!(err.contains("the corpus says 0"), "{err}");
    }

    #[test]
    fn the_twelve_corpus_rejections_still_qualify() {
        let contracts = contracts(&super::super::super::oracle::load_cases().unwrap());
        let split = outcomes(&agreeing_run(&contracts), &contracts).unwrap();
        let declared: BTreeSet<String> = contracts
            .iter()
            .filter(|(_, contract)| contract.declared_error.is_some())
            .map(|(id, _)| id.clone())
            .collect();
        assert_eq!(declared.len(), 12);
        assert_eq!(split.expected_rejections, declared);
        assert_eq!(split.positive_matches.len(), contracts.len() - 12);
        assert!(split.failed.is_empty());
        // Each one still fails without its declared text.
        for id in &declared {
            let rows = agreeing_rows(id, &contracts[id]);
            let text = contracts[id].declared_error.as_deref().unwrap();
            let escaped = serde_json::to_string(text).unwrap();
            let tampered = rows.replace(&escaped[1..escaped.len() - 1], "something else");
            assert_ne!(rows, tampered, "{id}");
            let raw = agreeing_run(&contracts).replace(&rows, &tampered);
            assert!(outcomes(&raw, &contracts).is_err(), "{id}");
        }
    }
}
