//! Binding a report to its run provenance: the file the official evidence
//! hashes, checked field by field against that evidence and the raw input.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use super::{Measurement, RunIdentity};
use crate::evidence::RUN_PROVENANCE_SCHEMA;
use crate::report::utils::{sha256_bytes, sha256_hex};

/// Reads the run provenance, proves the evidence binds it, and returns its
/// run identity and elapsed time after checking it against the evidence.
pub(super) fn verify_run_provenance(
    path: &Path,
    recorded_sha256: &str,
    suite: &str,
    measurement: &Measurement,
    run: &Value,
    raw_file: &str,
    raw_text: &str,
) -> Result<(RunIdentity, u128)> {
    let bytes =
        fs::read(path).with_context(|| format!("read run provenance {}", path.display()))?;
    let actual_sha256 = sha256_bytes(&bytes);
    if actual_sha256 != recorded_sha256 {
        bail!(
            "run provenance {} has SHA-256 {actual_sha256}, but the official evidence records {recorded_sha256}",
            path.display()
        );
    }
    let provenance: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse run provenance {}", path.display()))?;
    let text = |key: &str| provenance.get(key).and_then(Value::as_str);
    if text("schema_version") != Some(RUN_PROVENANCE_SCHEMA) {
        bail!(
            "run provenance schema_version {:?}, expected {RUN_PROVENANCE_SCHEMA:?}",
            provenance.get("schema_version")
        );
    }
    if text("suite") != Some(suite) {
        bail!(
            "run provenance is for suite {:?}, not {suite}",
            provenance.get("suite")
        );
    }
    let expected = [
        ("target_binary", measurement.target.as_ref()),
        ("sqlite_binary", measurement.sqlite.as_ref()),
        ("redline_testing_binary", measurement.runner.as_ref()),
    ];
    for (prefix, executable) in expected {
        let executable = executable.context("official evidence executable")?;
        let version_key = match prefix {
            "redline_testing_binary" => "redline_testing_version".to_owned(),
            other => format!("{}_version", other.trim_end_matches("_binary")),
        };
        for (key, value) in [
            (format!("{prefix}_path"), &executable.path),
            (format!("{prefix}_sha256"), &executable.sha256),
            (version_key, &executable.version),
        ] {
            if text(&key) != Some(value.as_str()) {
                bail!(
                    "run provenance {key} {:?} does not match the official evidence {value:?}",
                    provenance.get(&key)
                );
            }
        }
    }
    let raw_sha256 = sha256_hex(raw_text);
    let recorded_raw = provenance
        .get("output_file_hashes")
        .and_then(|hashes| hashes.get(raw_file))
        .and_then(Value::as_str);
    if recorded_raw != Some(raw_sha256.as_str()) {
        bail!(
            "run provenance output_file_hashes.{raw_file} {recorded_raw:?} does not match the raw input {raw_sha256}"
        );
    }
    let identity = run_identity(&provenance, "run provenance")?;
    let evidence_identity = run_identity(run, "official evidence")?;
    if identity != evidence_identity {
        let field = mismatched_field(&identity, &evidence_identity);
        bail!(
            "run provenance {field} does not match the official evidence ({:?} vs {:?})",
            provenance.get(field),
            run.get(field)
        );
    }
    if identity.source_dirty {
        bail!(
            "the run measured a dirty source tree (source_dirty = true; {}); \
             official reports need a run from a clean checkout",
            dirty_paths(&provenance)
        );
    }
    let elapsed_ns = provenance
        .get("elapsed_ns")
        .and_then(Value::as_u64)
        .map(u128::from)
        .context("run provenance records no elapsed_ns")?;
    Ok((identity, elapsed_ns))
}

fn run_identity(value: &Value, what: &str) -> Result<RunIdentity> {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty() && *text != "<unknown>")
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("{what} records no {key}"))
    };
    let digest = |key: &str, len: usize| {
        let value = text(key)?;
        if value.len() != len || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("{what} {key} is not a {len}-digit hex digest: {value:?}");
        }
        Ok(value)
    };
    Ok(RunIdentity {
        source_commit: digest("source_commit", 40)?,
        source_tree: digest("source_tree", 40)?,
        source_inputs_sha256: digest("source_inputs_sha256", 64)?,
        source_dirty: value
            .get("source_dirty")
            .and_then(Value::as_bool)
            .ok_or_else(|| anyhow!("{what} records no source_dirty"))?,
        corpus_sha256: digest("corpus_sha256", 64)?,
        assertion_policy_sha256: digest("assertion_policy_sha256", 64)?,
        oracle_build_stamp: text("oracle_build_stamp")?,
    })
}

fn mismatched_field(left: &RunIdentity, right: &RunIdentity) -> &'static str {
    if left.source_commit != right.source_commit {
        "source_commit"
    } else if left.source_tree != right.source_tree {
        "source_tree"
    } else if left.source_inputs_sha256 != right.source_inputs_sha256 {
        "source_inputs_sha256"
    } else if left.source_dirty != right.source_dirty {
        "source_dirty"
    } else if left.corpus_sha256 != right.corpus_sha256 {
        "corpus_sha256"
    } else if left.assertion_policy_sha256 != right.assertion_policy_sha256 {
        "assertion_policy_sha256"
    } else {
        "oracle_build_stamp"
    }
}

fn dirty_paths(provenance: &Value) -> String {
    let paths = provenance
        .get("source_dirty_paths")
        .and_then(Value::as_array)
        .map(|paths| {
            paths
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    if paths.is_empty() {
        "no paths recorded".to_owned()
    } else {
        format!("changed: {paths}")
    }
}
