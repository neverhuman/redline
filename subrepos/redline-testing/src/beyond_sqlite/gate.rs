//! Executed-case qualification and regression policy, separate from feature metadata.
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

mod cases;
use cases::outcomes;

#[derive(Deserialize)]
struct Baseline {
    schema_version: String,
    corpus_sha256: String,
    failed_cases: BTreeSet<String>,
    reference_settings: String,
    image_digest: String,
    server_binary_sha256: String,
}

#[derive(Serialize)]
struct Qualification {
    schema_version: &'static str,
    surface: &'static str,
    qualification: &'static str,
    regression: &'static str,
    required: usize,
    passed: usize,
    failed: usize,
    skipped: usize,
    unverified: usize,
    failed_cases: BTreeSet<String>,
    /// Cases the regression baseline lists as failing that now pass. Always
    /// empty on a green run; non-empty means the baseline needs pruning.
    newly_passing: BTreeSet<String>,
    source_commit: Value,
    source_dirty: bool,
    corpus_sha256: String,
    raw_sha256: String,
    provenance_sha256: String,
    policy_sha256: Option<String>,
    reference: Value,
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Verify immutable runner artifacts, write an honest qualification report, and
/// enforce either full qualification or an explicitly reviewed failure baseline.
pub fn check(raw_path: &Path, baseline_path: Option<&Path>, readme: Option<&Path>) -> Result<()> {
    let directory = raw_path
        .parent()
        .context("Postgres output directory missing")?;
    let raw = fs::read_to_string(raw_path)?;
    let provenance_bytes = fs::read(directory.join("beyond-sqlite-provenance.json"))?;
    let provenance: Value = serde_json::from_slice(&provenance_bytes)?;
    let hashes = provenance["output_file_hashes"]
        .as_object()
        .context("artifact hashes missing")?;
    for name in [
        "beyond_sqlite.raw.jsonl",
        "beyond-sqlite-summary.json",
        "beyond-sqlite-ranked.csv",
        "beyond-sqlite-coverage.csv",
        "beyond-sqlite-manifest.json",
    ] {
        let bytes = if name == "beyond_sqlite.raw.jsonl" {
            raw.as_bytes().to_vec()
        } else {
            fs::read(directory.join(name))?
        };
        ensure!(
            hashes.get(name).and_then(Value::as_str) == Some(hash(&bytes).as_str()),
            "artifact hash mismatch: {name}"
        );
    }
    for field in ["redline_testing_binary_sha256", "target_binary_sha256"] {
        let sha = provenance[field]
            .as_str()
            .context("binary identity missing")?;
        ensure!(
            sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
            "unknown binary hash: {field}"
        );
    }
    let corpus_hash = hash(super::oracle::MANIFEST.as_bytes());
    ensure!(
        provenance["corpus_sha256"] == corpus_hash,
        "corpus identity mismatch"
    );
    let contracts = cases::contracts(&super::oracle::load_cases()?);
    let required: BTreeSet<_> = contracts.keys().cloned().collect();
    let failures = outcomes(&raw, &contracts)?.failed;
    let reference = &provenance["reference"];
    ensure!(
        reference["settings"] == "160015|C|C|UTC",
        "unqualified PostgreSQL reference configuration"
    );
    ensure!(
        reference["image_digest"]
            == "sha256:efdf07c2f9d4df592783dcc8ea5f6db02efbf5f6452b527225ff5e58364570e9"
            || reference["server_binary_sha256"]
                == "201d8daff9a5bd9df70b820471b965c39f5a776d677f395232c477d051270722",
        "unknown PostgreSQL reference digest"
    );
    let baseline_bytes = baseline_path.map(fs::read).transpose()?;
    let baseline: Option<Baseline> = baseline_bytes
        .as_deref()
        .map(serde_json::from_slice)
        .transpose()?;
    // The ratchet has two directions. `failures.is_subset(...)` below catches a
    // failure set that grows. Nothing caught a case the baseline lists as
    // failing that has since started passing -- so a closed gap stays recorded
    // as broken, and the file overstates how much is left. That is exactly how
    // metadata/beyond_sqlite/skip-list.toml came to mark 29 cases "deferred"
    // while every one of them passed.
    let mut newly_passing: BTreeSet<String> = BTreeSet::new();
    let regression = if let Some(policy) = &baseline {
        ensure!(
            policy.schema_version == "redline-postgres-regression-v1",
            "unknown regression policy version"
        );
        ensure!(
            policy.corpus_sha256 == corpus_hash,
            "regression policy uses a different corpus"
        );
        ensure!(
            policy.failed_cases.is_subset(&required),
            "regression policy contains unknown cases"
        );
        ensure!(
            reference["settings"] == policy.reference_settings,
            "reference settings changed"
        );
        ensure!(
            reference["image_digest"] == policy.image_digest
                || reference["server_binary_sha256"] == policy.server_binary_sha256,
            "unknown reference digest"
        );
        newly_passing = policy.failed_cases.difference(&failures).cloned().collect();
        failures.is_subset(&policy.failed_cases)
    } else {
        failures.is_empty()
    };
    let report = Qualification {
        schema_version: "redline-postgres-qualification-v1",
        surface: "PostgreSQL 16.15 SQL through the RedlineDB shell; wire protocol unverified",
        qualification: if failures.is_empty() {
            "passed"
        } else {
            "failed"
        },
        regression: if regression { "passed" } else { "failed" },
        required: required.len(),
        passed: required.len() - failures.len(),
        failed: failures.len(),
        skipped: 0,
        unverified: 0,
        failed_cases: failures,
        newly_passing: newly_passing.clone(),
        source_commit: provenance["source_commit"].clone(),
        source_dirty: provenance["source_dirty"].as_bool().unwrap_or(true),
        corpus_sha256: corpus_hash,
        raw_sha256: hash(raw.as_bytes()),
        provenance_sha256: hash(&provenance_bytes),
        policy_sha256: baseline_bytes.as_deref().map(hash),
        reference: reference.clone(),
    };
    fs::write(
        directory.join("postgres-qualification.json"),
        serde_json::to_string_pretty(&report)? + "\n",
    )?;
    let markdown = format!(
        "PostgreSQL **16.15** SQL-shell corpus: **{} / {} passed**, **{} failed**, **0 skipped**. Corpus qualification: **{}**. Regression gate: **{}**.\n\nAll corpus cases run in CI; known failures remain failures. PostgreSQL wire/client and full application compatibility remain unverified. Source: `{}`{}; corpus SHA-256: `{}`.\n",
        report.passed,
        report.required,
        report.failed,
        report.qualification,
        report.regression,
        report.source_commit.as_str().unwrap_or("unrecorded"),
        if report.source_dirty {
            " (dirty workspace)"
        } else {
            ""
        },
        report.corpus_sha256
    );
    fs::write(directory.join("postgres-progress.md"), &markdown)?;
    ensure!(
        regression,
        "PostgreSQL gate failed: {} of {} cases failed; see postgres-qualification.json",
        report.failed,
        report.required
    );
    ensure!(
        newly_passing.is_empty(),
        "regression baseline lists failures that now pass: {} case(s). \
         Remove them from the baseline so it stops claiming they are broken: {}",
        newly_passing.len(),
        newly_passing.iter().cloned().collect::<Vec<_>>().join(", ")
    );
    if let Some(path) = readme {
        let text = fs::read_to_string(path)?;
        let start = "<!-- POSTGRES_PARITY_START -->";
        let end = "<!-- POSTGRES_PARITY_END -->";
        let (before, rest) = text
            .split_once(start)
            .context("README Postgres start marker missing")?;
        let (_, after) = rest
            .split_once(end)
            .context("README Postgres end marker missing")?;
        fs::write(path, format!("{before}{start}\n{markdown}{end}{after}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::cases::{agreeing_run, contracts};
    use super::*;
    // A complete synthetic bundle lets us exercise artifact verification without
    // a live server. It does not constitute compatibility evidence.
    fn bundle() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let raw = agreeing_run(&contracts(&super::super::oracle::load_cases().unwrap()));
        let mut hashes = serde_json::Map::new();
        for (name, text) in [
            ("beyond_sqlite.raw.jsonl", raw.as_str()),
            ("beyond-sqlite-summary.json", "{}"),
            ("beyond-sqlite-ranked.csv", "header\n"),
            ("beyond-sqlite-coverage.csv", "header\n"),
            ("beyond-sqlite-manifest.json", "{}"),
        ] {
            fs::write(dir.path().join(name), text).unwrap();
            hashes.insert(name.into(), Value::String(hash(text.as_bytes())));
        }
        let provenance = serde_json::json!({
            "output_file_hashes": hashes,
            "redline_testing_binary_sha256": "a".repeat(64),
            "target_binary_sha256": "b".repeat(64),
            "corpus_sha256": hash(super::super::oracle::MANIFEST.as_bytes()),
            "source_commit": "c".repeat(40),
            "reference": {
                "settings": "160015|C|C|UTC",
                "image_digest": "sha256:efdf07c2f9d4df592783dcc8ea5f6db02efbf5f6452b527225ff5e58364570e9"
            }
        });
        fs::write(
            dir.path().join("beyond-sqlite-provenance.json"),
            provenance.to_string(),
        )
        .unwrap();
        dir
    }

    /// Writes a regression baseline naming `failed_cases` as the expected
    /// failures, against a bundle in which every case passes.
    fn baseline(dir: &Path, failed_cases: &[&str]) -> std::path::PathBuf {
        let path = dir.join("regression.json");
        let policy = serde_json::json!({
            "schema_version": "redline-postgres-regression-v1",
            "corpus_sha256": hash(super::super::oracle::MANIFEST.as_bytes()),
            "failed_cases": failed_cases,
            "reference_settings": "160015|C|C|UTC",
            "image_digest": "sha256:efdf07c2f9d4df592783dcc8ea5f6db02efbf5f6452b527225ff5e58364570e9",
            "server_binary_sha256": "d".repeat(64),
        });
        fs::write(&path, policy.to_string()).unwrap();
        path
    }

    #[test]
    fn a_baseline_failure_that_now_passes_fails_the_gate() {
        let dir = bundle();
        let raw = dir.path().join("beyond_sqlite.raw.jsonl");
        // Every case in the bundle passes, so a baseline that claims one fails
        // is behind the live result. The subset ratchet alone is happy with this -- an empty set
        // is a subset of anything -- which is the hole being closed.
        let behind = baseline(dir.path(), &["BEYOND-CASE-20021"]);
        let err = check(&raw, Some(&behind), None).unwrap_err().to_string();
        assert!(
            err.contains("regression baseline lists failures that now pass"),
            "{err}"
        );
        assert!(err.contains("BEYOND-CASE-20021"), "{err}");

        // ... and the artifact records which ones, not just that some exist.
        let report: Value = serde_json::from_slice(
            &fs::read(dir.path().join("postgres-qualification.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            report["newly_passing"],
            serde_json::json!(["BEYOND-CASE-20021"])
        );

        // An exact baseline still passes, and records nothing newly passing.
        let exact = baseline(dir.path(), &[]);
        check(&raw, Some(&exact), None).unwrap();
        let report: Value = serde_json::from_slice(
            &fs::read(dir.path().join("postgres-qualification.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(report["newly_passing"], serde_json::json!([]));
    }

    #[test]
    fn tampered_artifacts_fail_without_rewriting_the_receipt() {
        let dir = bundle();
        let raw = dir.path().join("beyond_sqlite.raw.jsonl");
        check(&raw, None, None).unwrap();
        let provenance = dir.path().join("beyond-sqlite-provenance.json");
        let before = fs::read(&provenance).unwrap();
        fs::write(dir.path().join("beyond-sqlite-summary.json"), "tampered").unwrap();
        assert!(
            check(&raw, None, None)
                .unwrap_err()
                .to_string()
                .contains("artifact hash mismatch")
        );
        assert_eq!(fs::read(provenance).unwrap(), before);
    }

    #[test]
    fn an_unknown_oracle_or_an_unasserted_error_cannot_pass() {
        let dir = bundle();
        let provenance = dir.path().join("beyond-sqlite-provenance.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&provenance).unwrap()).unwrap();
        value["reference"]["image_digest"] = Value::String("unknown".into());
        fs::write(provenance, value.to_string()).unwrap();
        assert!(
            check(&dir.path().join("beyond_sqlite.raw.jsonl"), None, None)
                .unwrap_err()
                .to_string()
                .contains("unknown PostgreSQL reference digest")
        );
        let contracts = contracts(&super::super::oracle::load_cases().unwrap());
        let positive = contracts
            .iter()
            .find(|(_, contract)| contract.declared_error.is_none())
            .map(|(id, contract)| (id.clone(), contract.clone()))
            .unwrap();
        let only = std::collections::BTreeMap::from([positive.clone()]);
        assert!(
            outcomes(
                &cases::agreeing_rows(&positive.0, &positive.1)
                    .replace("exit_code\":0", "exit_code\":3"),
                &only
            )
            .is_err()
        );
    }
}
