//! Executed-case qualification and regression policy, separate from feature metadata.
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

mod cases;
mod policy;
mod render;
use cases::outcomes;

/// What an agreeing case establishes, in the words every report uses.
pub(crate) const COMPARISON: &str = "normalized SQL-shell transcript agreement";

/// The pinned PostgreSQL 16.15 reference (ci.yml `services.postgres`).
const REFERENCE_IMAGE: &str =
    "sha256:efdf07c2f9d4df592783dcc8ea5f6db02efbf5f6452b527225ff5e58364570e9";
const REFERENCE_SERVER_BINARY: &str =
    "201d8daff9a5bd9df70b820471b965c39f5a776d677f395232c477d051270722";

/// What a check requires before its result may be published (PG-05).
#[derive(Debug, Clone, Default)]
pub struct Publication {
    /// The commit the run must have measured. A provenance that records any
    /// other commit fails the check.
    pub expected_source_commit: Option<String>,
    /// Fail unless the evidence is release evidence (see `blockers`), even
    /// when no README is written.
    pub require_clean: bool,
}

/// Why this evidence is not release evidence; empty when it is. Writing the
/// README always requires an empty list.
fn blockers(provenance: &Value, reference: &Value) -> Vec<String> {
    let mut blockers = Vec::new();
    if !provenance["source_commit"]
        .as_str()
        .is_some_and(|commit| commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        blockers.push("the run recorded no source commit".to_owned());
    }
    if provenance["source_dirty"].as_bool() != Some(false) {
        let paths = provenance["source_dirty_paths"]
            .as_array()
            .map(|paths| {
                paths
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_default();
        blockers.push(format!(
            "the source tree was dirty or unknown when the run started [{paths}]"
        ));
    }
    let measured_image = reference["image_digest"] == REFERENCE_IMAGE
        && reference["image_digest_source"] == "measured";
    if !measured_image && reference["server_binary_sha256"] != REFERENCE_SERVER_BINARY {
        blockers.push(
            "the PostgreSQL reference image digest was asserted, not measured from the running container"
                .to_owned(),
        );
    }
    blockers
}

/// The qualification report (`postgres-qualification.json`). v2 splits
/// `passed` into row matches and expected rejections, and `failed` into
/// declared-unsupported refusals and mismatches.
#[derive(Serialize)]
struct Qualification {
    schema_version: &'static str,
    surface: &'static str,
    comparison: &'static str,
    comparator_version: &'static str,
    not_covered: [&'static str; 7],
    capability_matrix: &'static str,
    qualification: &'static str,
    regression: &'static str,
    required: usize,
    /// `positive_matches` plus the expected rejections.
    passed: usize,
    /// The declared-unsupported refusals plus the mismatches.
    failed: usize,
    skipped: usize,
    unverified: usize,
    /// Both engines exited 0 and printed the same normalized rows.
    positive_matches: usize,
    /// Declared rejections both engines refused, the target with the
    /// declared text and after its setup ran on its own.
    expected_rejections: BTreeSet<String>,
    /// PostgreSQL succeeded; the target refused with `unsupported
    /// capability:`. Failures, ratcheted by the policy's
    /// `declared_unsupported`.
    declared_unsupported: BTreeSet<String>,
    /// Every other failure, ratcheted by the policy's `failed_cases`.
    mismatches: BTreeSet<String>,
    /// `declared_unsupported` and `mismatches` together.
    failed_cases: BTreeSet<String>,
    /// Cases the regression baseline lists as failing that now pass. Always
    /// empty on a green run; non-empty means the baseline needs pruning.
    newly_passing: BTreeSet<String>,
    source_commit: Value,
    source_dirty: bool,
    /// The commit the check was told to expect, if any.
    expected_source_commit: Option<String>,
    /// Release evidence: a recorded commit, a clean source tree and a
    /// measured reference identity. Only publishable evidence may write the
    /// README.
    publishable: bool,
    publication_blockers: Vec<String>,
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
pub fn check(
    raw_path: &Path,
    baseline_path: Option<&Path>,
    readme: Option<&Path>,
    publication: &Publication,
) -> Result<()> {
    let directory = raw_path
        .parent()
        .context("Postgres output directory missing")?;
    let raw = fs::read_to_string(raw_path)?;
    let provenance_bytes = fs::read(directory.join("beyond-sqlite-provenance.json"))?;
    let provenance: Value = serde_json::from_slice(&provenance_bytes)?;
    ensure!(
        provenance["schema_version"] == super::taxonomy::PROVENANCE_SCHEMA,
        "Postgres provenance schema {} is not {}",
        provenance["schema_version"],
        super::taxonomy::PROVENANCE_SCHEMA
    );
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
    let declared_rejections: BTreeSet<String> = contracts
        .iter()
        .filter(|(_, contract)| contract.declared_error.is_some())
        .map(|(id, _)| id.clone())
        .collect();
    let split = outcomes(&raw, &contracts)?;
    let failures = split.failed();
    let reference = &provenance["reference"];
    ensure!(
        reference["settings"] == "160015|C|C|UTC",
        "unqualified PostgreSQL reference configuration"
    );
    ensure!(
        reference["image_digest"] == REFERENCE_IMAGE
            || reference["server_binary_sha256"] == REFERENCE_SERVER_BINARY,
        "unknown PostgreSQL reference digest"
    );
    let publication_blockers = blockers(&provenance, reference);
    let baseline_bytes = baseline_path.map(fs::read).transpose()?;
    let baseline: Option<policy::Baseline> = baseline_bytes
        .as_deref()
        .map(serde_json::from_slice)
        .transpose()
        .context("read the PostgreSQL regression policy")?;
    let (regression, newly_passing) = match &baseline {
        Some(baseline) => {
            let verdict = policy::ratchet(
                baseline,
                &split,
                &required,
                &declared_rejections,
                &corpus_hash,
                reference,
            )?;
            (verdict.passed, verdict.newly_passing)
        }
        None => (failures.is_empty(), BTreeSet::new()),
    };
    let report = Qualification {
        schema_version: "redline-postgres-qualification-v2",
        surface: "PostgreSQL 16.15 SQL-shell corpus through the redlinedb CLI (REDLINEDB_RESULT_DIALECT=postgres, fresh :memory: per case)",
        comparison: COMPARISON,
        comparator_version: super::transcript::COMPARATOR_VERSION,
        not_covered: render::NOT_COVERED,
        capability_matrix: "metadata/beyond_sqlite/postgres-capabilities.json",
        qualification: if failures.is_empty() {
            "passed"
        } else {
            "failed"
        },
        regression: if regression { "passed" } else { "failed" },
        required: required.len(),
        passed: split.positive_matches.len() + split.expected_rejections.len(),
        failed: failures.len(),
        skipped: 0,
        unverified: 0,
        positive_matches: split.positive_matches.len(),
        expected_rejections: split.expected_rejections.clone(),
        declared_unsupported: split.declared_unsupported.clone(),
        mismatches: split.mismatches.clone(),
        failed_cases: failures,
        newly_passing: newly_passing.clone(),
        source_commit: provenance["source_commit"].clone(),
        source_dirty: provenance["source_dirty"].as_bool().unwrap_or(true),
        expected_source_commit: publication.expected_source_commit.clone(),
        publishable: publication_blockers.is_empty(),
        publication_blockers: publication_blockers.clone(),
        corpus_sha256: corpus_hash,
        raw_sha256: hash(raw.as_bytes()),
        provenance_sha256: hash(&provenance_bytes),
        policy_sha256: baseline_bytes.as_deref().map(hash),
        reference: reference.clone(),
    };
    ensure!(
        report.passed + report.failed == report.required,
        "PostgreSQL outcomes do not cover the corpus: {} passed + {} failed != {}",
        report.passed,
        report.failed,
        report.required
    );
    fs::write(
        directory.join("postgres-qualification.json"),
        serde_json::to_string_pretty(&report)? + "\n",
    )?;
    let markdown = render::markdown(&report);
    let mut progress = markdown.clone();
    if !publication_blockers.is_empty() {
        progress.push_str(&format!(
            "\nNot release evidence: {}.\n",
            publication_blockers.join("; ")
        ));
    }
    fs::write(directory.join("postgres-progress.md"), &progress)?;
    if let Some(expected) = &publication.expected_source_commit {
        ensure!(
            report.source_commit.as_str() == Some(expected.as_str()),
            "the run measured source commit {}, not the expected {expected}",
            report.source_commit
        );
    }
    ensure!(
        regression,
        "PostgreSQL gate failed: {} of {} cases failed ({} declared unsupported, {} mismatches); \
         see postgres-qualification.json",
        report.failed,
        report.required,
        report.declared_unsupported.len(),
        report.mismatches.len()
    );
    ensure!(
        newly_passing.is_empty(),
        "regression baseline lists failures that now pass: {} case(s). \
         Remove them from the baseline so it stops claiming they are broken: {}",
        newly_passing.len(),
        newly_passing.iter().cloned().collect::<Vec<_>>().join(", ")
    );
    ensure!(
        !publication.require_clean || publication_blockers.is_empty(),
        "PostgreSQL evidence is not release evidence: {}",
        publication_blockers.join("; ")
    );
    if let Some(path) = readme {
        ensure!(
            publication_blockers.is_empty(),
            "refusing to write the README Postgres block from evidence that is not release evidence: {}",
            publication_blockers.join("; ")
        );
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
mod tests;
