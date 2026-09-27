//! Completion markers, as the evidence processor checks them (SQ-09).
//!
//! The runner streams each SQLite-shell suite's records to its raw file as
//! cases complete, and writes `<raw>.complete.json` only when the suite
//! finished. A suite is published only when that marker is declared and
//! hashed by the official evidence, and certifies exactly the raw file's
//! bytes, record count and case count: records left by an interrupted run
//! are readable, never publishable.

use std::{collections::BTreeSet, fs, path::Path};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The suites whose records the runner streams.
pub(crate) const STREAMED_SUITES: [&str; 3] = ["sqlite_parity", "memory", "rql_phase1"];
const COMPLETION_SCHEMA: &str = "redline-testing-raw-complete-v1";

/// The marker at `completion_path`, checked against the raw file at
/// `raw_path` (both relative to `root`).
pub(crate) fn check_suite(
    root: &Path,
    suite: &str,
    raw_path: &str,
    completion_path: &str,
) -> Result<Value> {
    let marker_file = root.join(completion_path);
    let marker: Value = serde_json::from_slice(
        &fs::read(&marker_file).with_context(|| format!("read {}", marker_file.display()))?,
    )
    .with_context(|| format!("parse {}", marker_file.display()))?;
    if marker["schema_version"] != COMPLETION_SCHEMA {
        bail!(
            "suite {suite} completion marker has schema_version {}, expected {COMPLETION_SCHEMA:?}",
            marker["schema_version"]
        );
    }
    if marker["suite"] != suite {
        bail!(
            "suite {suite} completion marker names suite {}",
            marker["suite"]
        );
    }
    let raw_file = root.join(raw_path);
    let raw = fs::read(&raw_file).with_context(|| format!("read {}", raw_file.display()))?;
    let raw_sha256 = format!("{:x}", Sha256::digest(&raw));
    if marker["raw_sha256"].as_str() != Some(raw_sha256.as_str()) {
        bail!(
            "suite {suite} completion marker certifies raw sha256 {}, but {raw_path} is {raw_sha256}",
            marker["raw_sha256"]
        );
    }
    let records = raw.iter().filter(|byte| **byte == b'\n').count();
    let mut cases = BTreeSet::new();
    for (index, line) in String::from_utf8_lossy(&raw).lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(line)
            .with_context(|| format!("parse {raw_path} line {}", index + 1))?;
        let case_id = record["case_id"]
            .as_str()
            .ok_or_else(|| anyhow!("{raw_path} line {} has no case_id", index + 1))?;
        cases.insert(case_id.to_owned());
    }
    for (field, actual) in [("records", records), ("cases", cases.len())] {
        if marker[field].as_u64() != u64::try_from(actual).ok() {
            bail!(
                "suite {suite} completion marker certifies {} {field}, but {raw_path} has {actual}",
                marker[field]
            );
        }
    }
    Ok(marker)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn marker(raw: &str, records: usize, cases: usize) -> Value {
        json!({
            "schema_version": COMPLETION_SCHEMA,
            "suite": "memory",
            "raw_file": "memory.raw.jsonl",
            "records": records,
            "cases": cases,
            "raw_sha256": format!("{:x}", Sha256::digest(raw.as_bytes())),
        })
    }

    #[test]
    fn completion_marker_must_certify_the_raw_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let raw = "{\"case_id\":\"00001\"}\n{\"case_id\":\"00001\"}\n{\"case_id\":\"00002\"}\n";
        fs::write(dir.path().join("memory.raw.jsonl"), raw).expect("raw");
        let check = |value: &Value| {
            fs::write(dir.path().join("marker.json"), value.to_string()).expect("marker");
            check_suite(dir.path(), "memory", "memory.raw.jsonl", "marker.json")
        };
        check(&marker(raw, 3, 2)).expect("a finished run's marker");
        // Records appended, or lost, after the marker was written.
        let mut other = marker(raw, 3, 2);
        other["raw_sha256"] = json!("0".repeat(64));
        let error = check(&other).expect_err("other bytes");
        assert!(
            format!("{error:#}").contains("certifies raw sha256"),
            "{error:#}"
        );
        for (records, cases) in [(2, 2), (3, 3)] {
            assert!(check(&marker(raw, records, cases)).is_err());
        }
        let mut other = marker(raw, 3, 2);
        other["suite"] = json!("sqlite_parity");
        assert!(check(&other).is_err());
        // No marker at all: the run did not finish.
        assert!(check_suite(dir.path(), "memory", "memory.raw.jsonl", "absent.json").is_err());
    }
}
