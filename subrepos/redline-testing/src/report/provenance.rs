//! The identity a report publishes (SQ-03/L-03/R4-03).
//!
//! Every measured identity (target, SQLite oracle, runner, source tree,
//! corpus, verdict rules, oracle build) comes from the run: its official
//! evidence and its own run provenance, cross-checked against each other and
//! against every raw sample. Nothing is probed on the rendering host: no
//! `PATH`, no `REDLINE_TESTING_*_BIN`, no Git. The report records its own
//! step separately in report-provenance.json.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::Value;

use super::evidence::{official_suite_entry, recorded_provenance_sha256, recorded_raw_file_name};
use super::types::{RawRecord, ReportOptions};
use super::utils::{sha256_bytes, sha256_hex};
use crate::evidence::RUN_PROVENANCE_SCHEMA;

pub(crate) const REPORT_PROVENANCE_SCHEMA: &str = "redline-testing-report-provenance-v1";

/// Why a historical report carries no run provenance.
const HISTORICAL_NOTE: &str = "historical run: it predates run provenance, so its source tree, source inputs, corpus hash, assertion policy and oracle build stamp were not recorded; the run provenance file the evidence names was not retained. Measured identities come from the processed official evidence.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ReportMode {
    /// Official evidence plus the run provenance it binds.
    Official,
    /// Official evidence from a run that recorded no run provenance.
    Historical,
    /// No official evidence; never committed.
    LocalDiagnostics,
}

/// One executable the run measured with, as the run recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Executable {
    pub(crate) path: String,
    pub(crate) sha256: String,
    pub(crate) version: String,
}

/// Source, corpus, verdict-rule and oracle-build identity of a run with
/// run provenance. Every field is required in official mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RunIdentity {
    pub(crate) source_commit: String,
    pub(crate) source_tree: String,
    pub(crate) source_inputs_sha256: String,
    pub(crate) source_dirty: bool,
    pub(crate) corpus_sha256: String,
    pub(crate) assertion_policy_sha256: String,
    pub(crate) oracle_build_stamp: String,
}

/// The run provenance a report derives from, or why there is none.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ParentRunProvenance {
    /// The run-provenance file as passed to `report`.
    pub(crate) path: Option<String>,
    /// Its SHA-256, as recorded by the official evidence.
    pub(crate) sha256: Option<String>,
    /// Whether the file itself was available and verified.
    pub(crate) retained: bool,
}

/// What was measured, with which executables, from which source.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Measurement {
    pub(crate) runner: Option<Executable>,
    pub(crate) target: Option<Executable>,
    pub(crate) sqlite: Option<Executable>,
    pub(crate) run: Option<RunIdentity>,
    /// The suite's elapsed time from the run provenance.
    pub(crate) elapsed_ns: Option<u128>,
}

/// The validated identity a report renders from.
#[derive(Debug, Clone)]
pub(crate) struct ReportIdentity {
    pub(crate) mode: ReportMode,
    pub(crate) measurement: Measurement,
    pub(crate) parent: ParentRunProvenance,
    /// SHA-256 of the run's official-evidence.json (`source_sha256`).
    pub(crate) run_evidence_sha256: Option<String>,
    /// SHA-256 of the evidence file passed to `report`.
    pub(crate) processed_evidence_sha256: Option<String>,
}

impl ReportIdentity {
    pub(crate) fn note(&self) -> Option<&'static str> {
        (self.mode == ReportMode::Historical).then_some(HISTORICAL_NOTE)
    }
}

/// The renderer that wrote a report. `report --check` does not compare it:
/// the check proves the committed outputs are what this renderer produces
/// from the committed inputs, whichever build of it wrote them.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Renderer {
    pub(crate) version: String,
    pub(crate) binary_sha256: String,
}

/// report-provenance.json: the rendering step, bound to its parent run.
/// It holds no Git state: the measured source identity is the run's.
#[derive(Debug, Serialize)]
pub(crate) struct ReportProvenanceJson {
    pub(crate) schema_version: &'static str,
    pub(crate) suite: String,
    pub(crate) mode: ReportMode,
    pub(crate) note: Option<&'static str>,
    pub(crate) measurement: Measurement,
    pub(crate) parent_run_provenance: ParentRunProvenance,
    pub(crate) run_evidence_sha256: Option<String>,
    pub(crate) processed_evidence_sha256: Option<String>,
    pub(crate) raw_sha256: String,
    pub(crate) renderer: Renderer,
    pub(crate) command_line: Vec<String>,
    pub(crate) updated_date: String,
    pub(crate) output_file_hashes: BTreeMap<String, String>,
}

/// Loads and cross-checks the identity behind `raw_records`.
pub(crate) fn load_report_identity(
    options: &ReportOptions,
    raw_text: &str,
    raw_records: &[RawRecord],
) -> Result<ReportIdentity> {
    let Some(evidence_path) = options.official_evidence.as_deref() else {
        if options.run_provenance.is_some() || options.historical_run {
            bail!(
                "--run-provenance and --historical-run describe official evidence; pass --official-evidence"
            );
        }
        return Ok(ReportIdentity {
            mode: ReportMode::LocalDiagnostics,
            measurement: Measurement {
                runner: None,
                target: None,
                sqlite: None,
                run: None,
                elapsed_ns: None,
            },
            parent: ParentRunProvenance {
                path: None,
                sha256: None,
                retained: false,
            },
            run_evidence_sha256: None,
            processed_evidence_sha256: None,
        });
    };
    let evidence_bytes = fs::read(evidence_path)
        .with_context(|| format!("read official evidence {}", evidence_path.display()))?;
    let evidence: Value = serde_json::from_slice(&evidence_bytes)
        .with_context(|| format!("parse official evidence {}", evidence_path.display()))?;
    // Processed evidence nests the run's own official-evidence.json.
    let run = evidence.get("official_evidence").unwrap_or(&evidence);
    let runner = executable(run, "runner", ["binary_path", "binary_sha256", "version"])?;
    let target = executable(run, "target", ["path", "sha256", "version"])?;
    let sqlite = executable(run, "sqlite", ["path", "sha256", "version"])?;
    check_raw_identity(&options.suite, raw_records, &target, &sqlite)?;

    let recorded_schema = run
        .get("run_provenance_schema")
        .and_then(Value::as_str)
        .map(str::to_owned);
    // Each suite entry names its own provenance file's schema. Only a run
    // provenance can be bound, and a suite with another kind of provenance
    // (beyond_sqlite's) is not a run that predates one either.
    if let Some(schema) = official_suite_entry(run, &options.suite)
        .and_then(|entry| entry.get("provenance_schema"))
        .and_then(Value::as_str)
        && schema != RUN_PROVENANCE_SCHEMA
    {
        bail!(
            "official evidence records suite {}'s provenance as {schema:?}, which `report` cannot \
             bind (it binds {RUN_PROVENANCE_SCHEMA:?}); the PostgreSQL results of beyond_sqlite \
             are checked and published by `check-postgres`",
            options.suite
        );
    }
    let recorded_sha256 = recorded_provenance_sha256(&evidence, &options.suite);
    let mut identity = ReportIdentity {
        mode: ReportMode::Historical,
        measurement: Measurement {
            runner: Some(runner),
            target: Some(target),
            sqlite: Some(sqlite),
            run: None,
            elapsed_ns: None,
        },
        parent: ParentRunProvenance {
            path: None,
            sha256: recorded_sha256.as_ref().ok().cloned(),
            retained: false,
        },
        run_evidence_sha256: evidence
            .get("source_sha256")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| std::ptr::eq(run, &evidence).then(|| sha256_bytes(&evidence_bytes))),
        processed_evidence_sha256: Some(sha256_bytes(&evidence_bytes)),
    };
    match (options.run_provenance.as_deref(), options.historical_run) {
        (Some(_), true) => bail!("pass either --run-provenance or --historical-run, not both"),
        (None, false) => bail!(
            "a report from official evidence needs --run-provenance <run-provenance.json>; \
             --historical-run is only for evidence from a run that recorded none"
        ),
        (None, true) => {
            if let Some(schema) = recorded_schema {
                bail!(
                    "official evidence {} records run provenance ({schema}); \
                     pass --run-provenance instead of --historical-run",
                    evidence_path.display()
                );
            }
        }
        (Some(path), false) => {
            let Some(schema) = recorded_schema else {
                bail!(
                    "official evidence {} records no run_provenance_schema, so no run provenance \
                     is bound to it; render it with --historical-run",
                    evidence_path.display()
                );
            };
            if schema != RUN_PROVENANCE_SCHEMA {
                bail!(
                    "official evidence run_provenance_schema {schema:?}, expected {RUN_PROVENANCE_SCHEMA:?}"
                );
            }
            let recorded_sha256 = recorded_sha256
                .context("official evidence records no run provenance hash for this suite")?;
            let raw_file = recorded_raw_file_name(&evidence, &options.suite);
            let (run_identity, elapsed_ns) = verify_run_provenance(
                path,
                &recorded_sha256,
                &options.suite,
                &identity.measurement,
                run,
                &raw_file,
                raw_text,
            )?;
            identity.mode = ReportMode::Official;
            identity.measurement.run = Some(run_identity);
            identity.measurement.elapsed_ns = Some(elapsed_ns);
            identity.parent = ParentRunProvenance {
                path: Some(path.display().to_string()),
                sha256: Some(recorded_sha256),
                retained: true,
            };
        }
    }
    Ok(identity)
}

/// `section`'s (path, sha256, version) from the run evidence; missing,
/// empty or `<unknown>` values and malformed digests are errors.
fn executable(run: &Value, section: &str, keys: [&str; 3]) -> Result<Executable> {
    let [path, sha256, version] = keys.map(|key| {
        run.get(section)
            .and_then(|entry| entry.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty() && *text != "<unknown>")
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("official evidence {section}.{key} is missing or unknown"))
    });
    let sha256 = sha256?;
    if !is_sha256(&sha256) {
        bail!(
            "official evidence {section}.{} is not a SHA-256 digest: {sha256:?}",
            keys[1]
        );
    }
    Ok(Executable {
        path: path?,
        sha256,
        version: version?,
    })
}

/// Every executed sample must name the executables the evidence names.
fn check_raw_identity(
    suite: &str,
    records: &[RawRecord],
    target: &Executable,
    sqlite: &Executable,
) -> Result<()> {
    // Beyond-SQLite records describe a PostgreSQL oracle, not sqlite3.
    let sqlite_reference = suite != "beyond_sqlite";
    for record in records.iter().filter(|record| record.status != "skipped") {
        let mut checks = vec![(
            "target_executable_sha256",
            &record.target_executable_sha256,
            &target.sha256,
        )];
        if sqlite_reference {
            checks.push((
                "reference_executable_sha256",
                &record.reference_executable_sha256,
                &sqlite.sha256,
            ));
            checks.push((
                "reference_version",
                &record.reference_version,
                &sqlite.version,
            ));
        }
        for (field, actual, expected) in checks {
            if actual != expected {
                bail!(
                    "raw record {} ({}): {field} {actual:?} does not match the official evidence {expected:?}",
                    record.case_id,
                    record.sample_role
                );
            }
        }
    }
    Ok(())
}

/// Reads the run provenance, proves the evidence binds it, and returns its
/// run identity and elapsed time after checking it against the evidence.
fn verify_run_provenance(
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

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
