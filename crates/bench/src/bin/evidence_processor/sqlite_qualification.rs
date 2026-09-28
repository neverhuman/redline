//! `sqlite-qualification.json`, as the evidence processor checks it (SQ-04).
//!
//! The runner's `check-sqlite` reduces each SQLite-shell suite to one
//! verdict per case, over complete and unique samples of exactly its
//! compiled-in manifest; this crate cannot link the runner, so
//! `scripts/process-redline-testing-evidence.sh` runs it and the processor
//! requires its output. The processor then holds it to what it can see:
//! the file comes from the verified runner and this run's evidence and
//! sample plan, each suite's raw bytes, the published counts equal the
//! corpus size, and the passed, failed and skipped ids are disjoint and
//! together are exactly the raw records' cases, failed and skipped ones
//! included.

use std::{collections::BTreeSet, fs, path::Path};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

pub(crate) const QUALIFICATION_FILE: &str = "sqlite-qualification.json";
const QUALIFICATION_SCHEMA: &str = "redline-testing-sqlite-qualification-v1";
pub(crate) const QUALIFIED_SUITES: [&str; 3] = ["sqlite_parity", "memory", "rql_phase1"];

/// Checks the file's provenance; returns it for the per-suite checks.
pub(crate) fn load(
    root: &Path,
    official: &Value,
    official_bytes: &[u8],
    expected_runner_sha: &str,
) -> Result<(Value, Value)> {
    let path = root.join(QUALIFICATION_FILE);
    let bytes = fs::read(&path).with_context(|| {
        format!(
            "read {} (written by `redline-testing check-sqlite`)",
            path.display()
        )
    })?;
    let qualification: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    if qualification["schema_version"] != QUALIFICATION_SCHEMA {
        bail!(
            "{QUALIFICATION_FILE} schema_version {}, expected {QUALIFICATION_SCHEMA:?}",
            qualification["schema_version"]
        );
    }
    if qualification["runner"]["binary_sha256"].as_str() != Some(expected_runner_sha) {
        bail!(
            "{QUALIFICATION_FILE} was written by runner {}, not the verified runner {expected_runner_sha}",
            qualification["runner"]["binary_sha256"]
        );
    }
    let official_sha256 = format!("{:x}", Sha256::digest(official_bytes));
    if qualification["official_evidence_sha256"].as_str() != Some(official_sha256.as_str()) {
        bail!(
            "{QUALIFICATION_FILE} checked official evidence {}, not this run's {official_sha256}",
            qualification["official_evidence_sha256"]
        );
    }
    for field in ["warmup", "repetitions"] {
        if qualification[field].as_u64().is_none() || qualification[field] != official[field] {
            bail!(
                "{QUALIFICATION_FILE} reduced with {field} {}, but the run used {}",
                qualification[field],
                official[field]
            );
        }
    }
    let summary = json!({
        "path": QUALIFICATION_FILE,
        "schema_version": QUALIFICATION_SCHEMA,
        "sha256": format!("{:x}", Sha256::digest(&bytes)),
        "corpus_sha256": qualification["corpus_sha256"],
    });
    Ok((qualification, summary))
}

/// One suite's reduction against its validated counts and raw records.
pub(crate) fn check_suite(
    qualification: &Value,
    suite: &str,
    validated: &Map<String, Value>,
    raw_path: &Path,
    expected_total: u64,
) -> Result<()> {
    let reduced = &qualification["suites"][suite];
    if !reduced.is_object() {
        bail!("{QUALIFICATION_FILE} has no {suite} suite");
    }
    let raw = fs::read(raw_path).with_context(|| format!("read {}", raw_path.display()))?;
    let raw_sha256 = format!("{:x}", Sha256::digest(&raw));
    if reduced["raw_sha256"].as_str() != Some(raw_sha256.as_str()) {
        bail!(
            "{QUALIFICATION_FILE} reduced {suite} raw sha256 {}, but {} is {raw_sha256}",
            reduced["raw_sha256"],
            raw_path.display()
        );
    }
    let count = |field: &str| {
        reduced[field]
            .as_u64()
            .ok_or_else(|| anyhow!("{QUALIFICATION_FILE} {suite} has no {field}"))
    };
    if count("total")? != expected_total {
        bail!(
            "{QUALIFICATION_FILE} reduced {suite} to {} cases, expected {expected_total}",
            reduced["total"]
        );
    }
    for field in ["total", "passed", "failed", "skipped"] {
        if Some(count(field)?) != validated.get(field).and_then(Value::as_u64) {
            bail!(
                "{QUALIFICATION_FILE} reduced {suite} to {field}={}, but the run published {:?}",
                reduced[field],
                validated.get(field)
            );
        }
    }
    let ids = |field: &str| -> Result<BTreeSet<String>> {
        let list = reduced[field]
            .as_array()
            .ok_or_else(|| anyhow!("{QUALIFICATION_FILE} {suite} has no {field}"))?;
        let set = list
            .iter()
            .map(|id| {
                id.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| anyhow!("{suite} {field} holds a non-string id {id}"))
            })
            .collect::<Result<BTreeSet<_>>>()?;
        let expected = count(field.trim_end_matches("_case_ids"))?;
        if set.len() != list.len() || set.len() as u64 != expected {
            bail!(
                "{QUALIFICATION_FILE} {suite} {field} holds {} ids ({} distinct), but counts {expected}",
                list.len(),
                set.len()
            );
        }
        Ok(set)
    };
    let (passed, failed, skipped) = (
        ids("passed_case_ids")?,
        ids("failed_case_ids")?,
        ids("skipped_case_ids")?,
    );
    if !passed.is_disjoint(&failed)
        || !passed.is_disjoint(&skipped)
        || !failed.is_disjoint(&skipped)
    {
        bail!("{QUALIFICATION_FILE} {suite} gives a case more than one verdict");
    }
    let (raw_all, raw_failed, raw_skipped) = raw_case_ids(&raw, raw_path)?;
    let reduced_all = passed
        .iter()
        .chain(&failed)
        .chain(&skipped)
        .cloned()
        .collect::<BTreeSet<_>>();
    for (what, reduced, raw) in [
        ("cases", &reduced_all, &raw_all),
        ("failed cases", &failed, &raw_failed),
        ("skipped cases", &skipped, &raw_skipped),
    ] {
        if reduced != raw {
            let missing = raw.difference(reduced).take(10).collect::<Vec<_>>();
            let extra = reduced.difference(raw).take(10).collect::<Vec<_>>();
            bail!(
                "{QUALIFICATION_FILE} {suite} {what} differ from the raw records: raw only {missing:?}; reduction only {extra:?}"
            );
        }
    }
    Ok(())
}

/// Every case id in raw records, and those with a failed or skipped record.
fn raw_case_ids(
    raw: &[u8],
    path: &Path,
) -> Result<(BTreeSet<String>, BTreeSet<String>, BTreeSet<String>)> {
    let (mut all, mut failed, mut skipped) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for (index, line) in String::from_utf8_lossy(raw).lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(line)
            .with_context(|| format!("parse {} line {}", path.display(), index + 1))?;
        let case_id = record["case_id"]
            .as_str()
            .ok_or_else(|| anyhow!("{} line {} has no case_id", path.display(), index + 1))?
            .to_owned();
        match record["status"].as_str() {
            Some("failed") => {
                failed.insert(case_id.clone());
            }
            Some("skipped") => {
                skipped.insert(case_id.clone());
            }
            _ => {}
        }
        all.insert(case_id);
    }
    Ok((all, failed, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNNER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    struct Run {
        dir: tempfile::TempDir,
        official: Value,
        official_bytes: Vec<u8>,
        qualification: Value,
        validated: Map<String, Value>,
    }

    impl Run {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let raw = concat!(
                "{\"case_id\":\"00001\",\"status\":\"passed\"}\n",
                "{\"case_id\":\"00002\",\"status\":\"passed\"}\n",
                "{\"case_id\":\"00002\",\"status\":\"failed\"}\n",
                "{\"case_id\":\"00003\",\"status\":\"skipped\"}\n",
            );
            fs::write(dir.path().join("raw.jsonl"), raw).expect("raw");
            let official = json!({"warmup": 1, "repetitions": 3});
            let official_bytes = official.to_string().into_bytes();
            let qualification = json!({
                "schema_version": QUALIFICATION_SCHEMA,
                "runner": {"binary_sha256": RUNNER},
                "official_evidence_sha256": format!("{:x}", Sha256::digest(&official_bytes)),
                "warmup": 1, "repetitions": 3,
                "suites": {"memory": {
                    "raw_sha256": format!("{:x}", Sha256::digest(raw.as_bytes())),
                    "total": 3, "passed": 1, "failed": 1, "skipped": 1,
                    "passed_case_ids": ["00001"], "failed_case_ids": ["00002"],
                    "skipped_case_ids": ["00003"],
                }},
            });
            let validated = json!({"total": 3, "passed": 1, "failed": 1, "skipped": 1})
                .as_object()
                .expect("object")
                .clone();
            Self {
                dir,
                official,
                official_bytes,
                qualification,
                validated,
            }
        }

        fn check(&self) -> Result<()> {
            fs::write(
                self.dir.path().join(QUALIFICATION_FILE),
                self.qualification.to_string(),
            )
            .expect("qualification");
            let (qualification, _) = load(
                self.dir.path(),
                &self.official,
                &self.official_bytes,
                RUNNER,
            )?;
            check_suite(
                &qualification,
                "memory",
                &self.validated,
                &self.dir.path().join("raw.jsonl"),
                3,
            )
        }
    }

    #[test]
    fn qualification_must_match_the_raw_records() {
        Run::new().check().expect("a consistent reduction");
        let broken: [(&str, fn(&mut Value)); 9] = [
            ("overlapping verdicts", |q| {
                q["suites"]["memory"]["passed_case_ids"] = json!(["00002"]);
            }),
            ("a replaced case", |q| {
                q["suites"]["memory"]["passed_case_ids"] = json!(["00009"]);
            }),
            ("a failed case called passed", |q| {
                q["suites"]["memory"]["failed_case_ids"] = json!(["00001"]);
                q["suites"]["memory"]["passed_case_ids"] = json!(["00002"]);
            }),
            ("ids that disagree with a count", |q| {
                q["suites"]["memory"]["passed_case_ids"] = json!(["00001", "00001"]);
            }),
            ("other raw bytes", |q| {
                q["suites"]["memory"]["raw_sha256"] = json!("0".repeat(64));
            }),
            ("another runner", |q| {
                q["runner"]["binary_sha256"] = json!("f".repeat(64));
            }),
            ("another run's evidence", |q| {
                q["official_evidence_sha256"] = json!("0".repeat(64));
            }),
            ("another sample plan", |q| {
                q["repetitions"] = json!(1);
            }),
            ("a missing suite", |q| {
                q["suites"] = json!({});
            }),
        ];
        for (what, edit) in broken {
            let mut run = Run::new();
            edit(&mut run.qualification);
            assert!(run.check().is_err(), "{what} was accepted");
        }
        // The published counts must be the reduction's, and the corpus size.
        let mut run = Run::new();
        run.validated.insert("passed".to_owned(), json!(2));
        assert!(run.check().is_err());
        let run = Run::new();
        fs::write(
            run.dir.path().join(QUALIFICATION_FILE),
            run.qualification.to_string(),
        )
        .expect("qualification");
        let (qualification, summary) =
            load(run.dir.path(), &run.official, &run.official_bytes, RUNNER).expect("load");
        assert_eq!(summary["schema_version"], QUALIFICATION_SCHEMA);
        assert!(
            check_suite(
                &qualification,
                "memory",
                &run.validated,
                &run.dir.path().join("raw.jsonl"),
                2_445
            )
            .is_err()
        );
    }
}
