use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::sqlite_parity::{
    BaselineSource, KNOWN_FAILURES_SCHEMA, RunSummary, SCOPE_POLICY_PATH, SCOPE_POLICY_SCHEMA,
};
use crate::{latency, report};

pub(crate) mod identity;
#[cfg(test)]
mod official_tests;
#[cfg(test)]
mod tests;

pub(crate) use identity::{RUN_PROVENANCE_SCHEMA, RunIdentity};

#[derive(Debug)]
pub struct EvidenceConfig {
    pub suite: String,
    pub output: PathBuf,
    pub target_bin: PathBuf,
    pub sqlite_bin: PathBuf,
    pub tmp_root: PathBuf,
    pub workers: String,
    pub repetitions: usize,
    pub warmup: usize,
    pub memory_samples: bool,
    /// Which engine ran first in each sample (BM3-04).
    pub measurement_order: crate::sqlite_parity::MeasurementOrder,
    pub command_line: Vec<String>,
    pub started_unix_ms: u128,
    pub ended_unix_ms: u128,
    pub summary: RunSummary,
    /// Captured once when the run started (`identity::capture`).
    pub run_identity: RunIdentity,
}

#[derive(Debug)]
pub struct OfficialSuiteEvidence {
    pub name: String,
    pub raw_path: PathBuf,
    pub summary_path: PathBuf,
    pub ranked_path: PathBuf,
    pub manifest_path: PathBuf,
    pub provenance_path: PathBuf,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    /// The failed case ids, for a suite whose summary lists them.
    pub failed_case_ids: Option<BTreeSet<String>>,
    /// The case ids the known-failures baseline lists for this suite.
    pub known_failure_ids: Option<BTreeSet<String>>,
    /// The case ids skipped under the scope policy (SQ-05).
    pub skipped_case_ids: Option<BTreeSet<String>>,
    /// The completion marker of `raw_path` (SQ-09), for a suite whose
    /// records the runner streams.
    pub completion_path: Option<PathBuf>,
}

impl OfficialSuiteEvidence {
    pub fn new(
        name: impl Into<String>,
        raw_path: PathBuf,
        summary_path: PathBuf,
        ranked_path: PathBuf,
        manifest_path: PathBuf,
        provenance_path: PathBuf,
        summary: &RunSummary,
    ) -> Self {
        Self {
            name: name.into(),
            raw_path,
            summary_path,
            ranked_path,
            manifest_path,
            provenance_path,
            total: summary.total,
            passed: summary.passed,
            failed: summary.failed,
            skipped: summary.skipped,
            failed_case_ids: Some(failed_case_ids(summary)),
            known_failure_ids: None,
            skipped_case_ids: Some(summary.skipped_case_ids.iter().cloned().collect()),
            completion_path: None,
        }
    }

    /// Declares the completion marker the runner wrote beside `raw_path`.
    pub fn with_completion_marker(mut self) -> Self {
        self.completion_path = Some(crate::sqlite_parity::completion_marker_path(&self.raw_path));
        self
    }

    /// Records the baseline's case ids for this suite beside its failures.
    pub fn with_known_failures(mut self, known_failure_ids: BTreeSet<String>) -> Self {
        self.known_failure_ids = Some(known_failure_ids);
        self
    }

    /// For a suite whose summary counts failures without listing them
    /// (beyond_sqlite; its gate writes postgres-qualification.json).
    pub fn without_case_ids(mut self) -> Self {
        self.failed_case_ids = None;
        self.skipped_case_ids = None;
        self
    }
}

fn failed_case_ids(summary: &RunSummary) -> BTreeSet<String> {
    summary
        .failures
        .iter()
        .map(|failure| failure.case_id.clone())
        .collect()
}

#[derive(Debug)]
pub struct OfficialEvidenceConfig {
    pub output_dir: PathBuf,
    pub all_output: PathBuf,
    pub all_manifest: PathBuf,
    pub target_bin: PathBuf,
    pub sqlite_bin: PathBuf,
    pub tmp_root: PathBuf,
    pub workers: String,
    pub repetitions: usize,
    pub warmup: usize,
    pub memory_samples: bool,
    pub command_line: Vec<String>,
    pub generated_at_unix_ms: u128,
    pub suites: Vec<OfficialSuiteEvidence>,
    /// The SQLite known-failures baseline the run was gated with.
    pub known_failures: Option<BaselineSource>,
    /// The scope policy compiled into the runner (SQ-05).
    pub scope_policy_sha256: String,
    /// Whether this was a `run --official`; only such evidence is
    /// publishable.
    pub official: bool,
    /// The bound on every engine run (SQ-09).
    pub case_timeout_ms: u128,
    pub max_output_bytes: usize,
    /// Captured once when the run started (`identity::capture`).
    pub run_identity: RunIdentity,
}

#[derive(Debug, Serialize)]
struct BinaryEvidence {
    path: String,
    sha256: String,
    version: String,
}

#[derive(Debug, Serialize)]
struct RunnerEvidence {
    binary_path: String,
    binary_sha256: String,
    release_binary_sha256: String,
    release_tarball_sha256: Option<String>,
    version: String,
}

#[derive(Debug, Serialize)]
struct OfficialSuiteJson {
    name: String,
    total: usize,
    passed: usize,
    failed: usize,
    skipped: usize,
    raw_path: String,
    summary_path: String,
    ranked_path: String,
    manifest_path: String,
    provenance_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    failed_case_ids: Option<BTreeSet<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    known_failure_ids: Option<BTreeSet<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skipped_case_ids: Option<BTreeSet<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completion_path: Option<String>,
}

/// The scope policy a run was selected with (SQ-05).
#[derive(Debug, Serialize)]
struct ScopePolicyJson {
    schema_version: &'static str,
    path: &'static str,
    sha256: String,
}

/// The known-failures baseline a run was gated with.
#[derive(Debug, Serialize)]
struct KnownFailuresJson {
    schema_version: &'static str,
    path: String,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct OfficialEvidenceJson {
    schema_version: String,
    /// The per-suite provenance files follow this schema; `report` requires
    /// them for any evidence that names one.
    run_provenance_schema: String,
    #[serde(flatten)]
    run_identity: RunIdentity,
    runner: RunnerEvidence,
    target: BinaryEvidence,
    sqlite: BinaryEvidence,
    suites: BTreeMap<String, OfficialSuiteJson>,
    sqlite_known_failures: Option<KnownFailuresJson>,
    sqlite_scope_policy: ScopePolicyJson,
    /// `official` for `run --official`, else `diagnostic` (not publishable).
    run_mode: &'static str,
    status: String,
    command_line: Vec<String>,
    generated_at_unix_ms: u128,
    output_file_hashes: BTreeMap<String, String>,
    workers: String,
    repetitions: usize,
    warmup: usize,
    memory_samples: bool,
    tmp_root: String,
    case_timeout_ms: u128,
    max_output_bytes: usize,
}

#[derive(Debug, Serialize)]
struct SummaryJson {
    suite: String,
    total_cases: usize,
    passed_cases: usize,
    failed_cases: usize,
    skipped_cases: usize,
    /// Every failed case id; failures are published, not hidden.
    failed_case_ids: BTreeSet<String>,
    skipped_case_ids: BTreeSet<String>,
    elapsed_ns: u128,
    measured_samples: usize,
    warmup_samples: usize,
    ranked_cases: usize,
    repetitions: usize,
    warmup: usize,
    measurement_boundary: String,
    ranked_schema: String,
}

#[derive(Debug, Serialize)]
struct ManifestJson {
    schema_version: String,
    suite: String,
    command_line: Vec<String>,
    workers: String,
    repetitions: usize,
    warmup: usize,
    memory_samples: bool,
    measurement_order: &'static str,
    output_files: BTreeMap<String, String>,
}

/// The run's own provenance for one suite. `report` stages it unchanged as
/// run-provenance.json and never rewrites it.
#[derive(Debug, Serialize)]
struct ProvenanceJson {
    schema_version: String,
    suite: String,
    #[serde(flatten)]
    run_identity: RunIdentity,
    target_binary_path: String,
    target_binary_sha256: String,
    target_version: String,
    redline_testing_binary_path: String,
    redline_testing_binary_sha256: String,
    redline_testing_release_binary_sha256: String,
    redline_testing_release_tarball_sha256: Option<String>,
    redline_testing_version: String,
    sqlite_binary_path: String,
    sqlite_binary_sha256: String,
    sqlite_version: String,
    command_line: Vec<String>,
    worker_count: String,
    repetitions: usize,
    warmup: usize,
    memory_samples: bool,
    tmp_root: String,
    os: String,
    arch: String,
    cpu: String,
    available_parallelism: usize,
    started_unix_ms: u128,
    ended_unix_ms: u128,
    /// Wall time of the suite run itself, from the runner's summary.
    elapsed_ns: u128,
    output_file_hashes: BTreeMap<String, String>,
}

pub fn write_sqlite_parity_evidence(config: EvidenceConfig) -> Result<()> {
    identity::still_unchanged(&config.run_identity, &config.sqlite_bin)?;
    let raw_text = fs::read_to_string(&config.output)
        .with_context(|| format!("read raw output {}", config.output.display()))?;
    let raw_records = report::parse_raw_records(&raw_text)?;
    let ranked = report::rank_cases(&raw_records)?;
    let output_dir = config
        .output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let summary_path = output_dir.join(suite_artifact_name(&config.suite, "summary.json"));
    let ranked_path = output_dir.join(suite_artifact_name(&config.suite, "ranked.csv"));
    let manifest_path = output_dir.join(suite_artifact_name(&config.suite, "manifest.json"));
    let provenance_path = output_dir.join(suite_artifact_name(&config.suite, "provenance.json"));

    let summary_json = serde_json::to_string_pretty(&SummaryJson {
        suite: config.suite.clone(),
        total_cases: config.summary.total,
        passed_cases: config.summary.passed,
        failed_cases: config.summary.failed,
        skipped_cases: config.summary.skipped,
        failed_case_ids: failed_case_ids(&config.summary),
        skipped_case_ids: config.summary.skipped_case_ids.iter().cloned().collect(),
        elapsed_ns: config.summary.elapsed.as_nanos(),
        measured_samples: raw_records
            .iter()
            .filter(|record| report::is_measured(record))
            .count(),
        warmup_samples: raw_records
            .iter()
            .filter(|record| record.sample_role == "warmup")
            .count(),
        ranked_cases: ranked.len(),
        repetitions: config.repetitions,
        warmup: config.warmup,
        measurement_boundary: latency::MEASUREMENT_BOUNDARY.to_owned(),
        ranked_schema: latency::RANKED_CSV_SCHEMA.to_owned(),
    })? + "\n";
    let ranked_csv = report::ranked_csv(&ranked);
    let manifest_json = serde_json::to_string_pretty(&ManifestJson {
        schema_version: "redline-testing-manifest-v1".to_owned(),
        suite: config.suite.clone(),
        command_line: config.command_line.clone(),
        workers: config.workers.clone(),
        repetitions: config.repetitions,
        warmup: config.warmup,
        memory_samples: config.memory_samples,
        measurement_order: config.measurement_order.as_str(),
        output_files: BTreeMap::from([
            ("raw".to_owned(), display_path(&config.output)),
            ("summary".to_owned(), display_path(&summary_path)),
            ("ranked".to_owned(), display_path(&ranked_path)),
            ("provenance".to_owned(), display_path(&provenance_path)),
        ]),
    })? + "\n";

    fs::write(&summary_path, summary_json)
        .with_context(|| format!("write {}", summary_path.display()))?;
    fs::write(&ranked_path, ranked_csv)
        .with_context(|| format!("write {}", ranked_path.display()))?;
    fs::write(&manifest_path, manifest_json)
        .with_context(|| format!("write {}", manifest_path.display()))?;

    let redline_testing_bin = std::env::current_exe().context("resolve current executable")?;
    let redline_testing_binary_sha256 = sha256_file(&redline_testing_bin)?;
    let release_binary_sha256 = env_sha("CI_REDLINE_TESTING_RELEASE_BINARY_SHA256")
        .or_else(|| env_sha("CI_REDLINE_TESTING_BIN_SHA256"))
        .unwrap_or_else(|| redline_testing_binary_sha256.clone());
    let raw_hash = sha256_file(&config.output)?;
    let output_hashes = BTreeMap::from([
        (file_name(&config.output), raw_hash.clone()),
        (display_path(&config.output), raw_hash),
        (file_name(&summary_path), sha256_file(&summary_path)?),
        (file_name(&ranked_path), sha256_file(&ranked_path)?),
        (file_name(&manifest_path), sha256_file(&manifest_path)?),
    ]);
    let provenance_json = serde_json::to_string_pretty(&ProvenanceJson {
        schema_version: RUN_PROVENANCE_SCHEMA.to_owned(),
        suite: config.suite,
        run_identity: config.run_identity,
        target_binary_path: canonical_display(&config.target_bin),
        target_binary_sha256: sha256_file(&resolve_executable_path(&config.target_bin)?)?,
        target_version: capture_version(&config.target_bin)?,
        redline_testing_binary_path: display_path(&redline_testing_bin),
        redline_testing_binary_sha256,
        redline_testing_release_binary_sha256: release_binary_sha256,
        redline_testing_release_tarball_sha256: env_sha(
            "CI_REDLINE_TESTING_RELEASE_TARBALL_SHA256",
        ),
        redline_testing_version: runner_version(),
        sqlite_binary_path: canonical_display(&config.sqlite_bin),
        sqlite_binary_sha256: sha256_file(&resolve_executable_path(&config.sqlite_bin)?)?,
        sqlite_version: capture_version(&config.sqlite_bin)?,
        command_line: config.command_line,
        worker_count: config.workers,
        repetitions: config.repetitions,
        warmup: config.warmup,
        memory_samples: config.memory_samples,
        tmp_root: display_path(&config.tmp_root),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        cpu: cpu_model(),
        available_parallelism: std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1),
        started_unix_ms: config.started_unix_ms,
        ended_unix_ms: config.ended_unix_ms,
        elapsed_ns: config.summary.elapsed.as_nanos(),
        output_file_hashes: output_hashes,
    })? + "\n";
    fs::write(&provenance_path, provenance_json)
        .with_context(|| format!("write {}", provenance_path.display()))?;
    Ok(())
}

pub fn suite_artifact_path(output_dir: &Path, suite: &str, base: &str) -> PathBuf {
    output_dir.join(suite_artifact_name(suite, base))
}

fn suite_artifact_name(suite: &str, base: &str) -> String {
    match suite {
        "sqlite_parity" | "all" => base.to_owned(),
        "rql_phase1" => format!("rql-phase1-{base}"),
        "memory" => format!("memory-{base}"),
        other => format!("{}-{base}", other.replace('_', "-")),
    }
}

pub fn write_official_evidence(config: OfficialEvidenceConfig) -> Result<()> {
    identity::still_unchanged(&config.run_identity, &config.sqlite_bin)?;
    let redline_testing_bin = std::env::current_exe().context("resolve current executable")?;
    let redline_testing_binary_sha256 = sha256_file(&redline_testing_bin)?;
    let release_binary_sha256 = env_sha("CI_REDLINE_TESTING_RELEASE_BINARY_SHA256")
        .or_else(|| env_sha("CI_REDLINE_TESTING_BIN_SHA256"))
        .unwrap_or_else(|| redline_testing_binary_sha256.clone());
    let runner = RunnerEvidence {
        binary_path: display_path(&redline_testing_bin),
        binary_sha256: redline_testing_binary_sha256,
        release_binary_sha256,
        release_tarball_sha256: env_sha("CI_REDLINE_TESTING_RELEASE_TARBALL_SHA256"),
        version: runner_version(),
    };
    let target = BinaryEvidence {
        path: canonical_display(&config.target_bin),
        sha256: sha256_file(&resolve_executable_path(&config.target_bin)?)?,
        version: capture_version(&config.target_bin)?,
    };
    let sqlite = BinaryEvidence {
        path: canonical_display(&config.sqlite_bin),
        sha256: sha256_file(&resolve_executable_path(&config.sqlite_bin)?)?,
        version: capture_version(&config.sqlite_bin)?,
    };

    let mut output_file_hashes = BTreeMap::new();
    insert_hash(
        &mut output_file_hashes,
        &config.output_dir,
        &config.all_output,
    )?;
    insert_hash(
        &mut output_file_hashes,
        &config.output_dir,
        &config.all_manifest,
    )?;

    for name in ["postgres-qualification.json", "postgres-progress.md"] {
        insert_hash(
            &mut output_file_hashes,
            &config.output_dir,
            &config.output_dir.join(name),
        )?;
    }
    let mut suites = BTreeMap::new();
    let mut failed = 0usize;
    for suite in config.suites {
        failed = failed.saturating_add(suite.failed);
        insert_hash(&mut output_file_hashes, &config.output_dir, &suite.raw_path)?;
        insert_hash(
            &mut output_file_hashes,
            &config.output_dir,
            &suite.summary_path,
        )?;
        insert_hash(
            &mut output_file_hashes,
            &config.output_dir,
            &suite.ranked_path,
        )?;
        insert_hash(
            &mut output_file_hashes,
            &config.output_dir,
            &suite.manifest_path,
        )?;
        insert_hash(
            &mut output_file_hashes,
            &config.output_dir,
            &suite.provenance_path,
        )?;
        if let Some(completion_path) = &suite.completion_path {
            insert_hash(&mut output_file_hashes, &config.output_dir, completion_path)?;
        }
        suites.insert(
            suite.name.clone(),
            OfficialSuiteJson {
                name: suite.name,
                total: suite.total,
                passed: suite.passed,
                failed: suite.failed,
                skipped: suite.skipped,
                raw_path: relative_display(&config.output_dir, &suite.raw_path),
                summary_path: relative_display(&config.output_dir, &suite.summary_path),
                ranked_path: relative_display(&config.output_dir, &suite.ranked_path),
                manifest_path: relative_display(&config.output_dir, &suite.manifest_path),
                provenance_path: relative_display(&config.output_dir, &suite.provenance_path),
                failed_case_ids: suite.failed_case_ids,
                known_failure_ids: suite.known_failure_ids,
                skipped_case_ids: suite.skipped_case_ids,
                completion_path: suite
                    .completion_path
                    .as_deref()
                    .map(|path| relative_display(&config.output_dir, path)),
            },
        );
    }

    let evidence = OfficialEvidenceJson {
        schema_version: "redline-testing-official-evidence-v1".to_owned(),
        run_provenance_schema: RUN_PROVENANCE_SCHEMA.to_owned(),
        run_identity: config.run_identity,
        runner,
        target,
        sqlite,
        suites,
        sqlite_known_failures: config.known_failures.map(|source| KnownFailuresJson {
            schema_version: KNOWN_FAILURES_SCHEMA,
            path: display_path(&source.path),
            sha256: source.sha256,
        }),
        sqlite_scope_policy: ScopePolicyJson {
            schema_version: SCOPE_POLICY_SCHEMA,
            path: SCOPE_POLICY_PATH,
            sha256: config.scope_policy_sha256,
        },
        run_mode: if config.official {
            "official"
        } else {
            "diagnostic"
        },
        status: if failed == 0 { "passed" } else { "failed" }.to_owned(),
        command_line: config.command_line,
        generated_at_unix_ms: config.generated_at_unix_ms,
        output_file_hashes,
        workers: config.workers,
        repetitions: config.repetitions,
        warmup: config.warmup,
        memory_samples: config.memory_samples,
        tmp_root: display_path(&config.tmp_root),
        case_timeout_ms: config.case_timeout_ms,
        max_output_bytes: config.max_output_bytes,
    };
    let evidence_path = config.output_dir.join("official-evidence.json");
    fs::write(
        &evidence_path,
        format!("{}\n", serde_json::to_string_pretty(&evidence)?),
    )
    .with_context(|| format!("write {}", evidence_path.display()))
}

fn insert_hash(
    output_file_hashes: &mut BTreeMap<String, String>,
    output_dir: &Path,
    path: &Path,
) -> Result<()> {
    output_file_hashes.insert(relative_display(output_dir, path), sha256_file(path)?);
    Ok(())
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(display_path)
        .unwrap_or_else(|_| display_path(path))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| display_path(path))
}

fn runner_version() -> String {
    format!("redline-testing {}", env!("CARGO_PKG_VERSION"))
}

pub fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn env_sha(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn capture_version(path: &Path) -> Result<String> {
    let resolved = resolve_executable_path(path)?;
    let output = Command::new(&resolved)
        .arg("--version")
        .output()
        .with_context(|| format!("run {} --version", resolved.display()))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn resolve_executable_path(path: &Path) -> Result<PathBuf> {
    if path.components().count() > 1 || path.is_absolute() {
        return fs::canonicalize(path)
            .with_context(|| format!("canonicalize executable {}", path.display()));
    }
    let Some(path_var) = std::env::var_os("PATH") else {
        anyhow::bail!("PATH is unset while resolving {}", path.display());
    };
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(path);
        if candidate.is_file() {
            return fs::canonicalize(&candidate)
                .with_context(|| format!("canonicalize executable {}", candidate.display()));
        }
    }
    anyhow::bail!("executable not found on PATH: {}", path.display())
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

fn canonical_display(path: &Path) -> String {
    resolve_executable_path(path)
        .map(|path| display_path(&path))
        .unwrap_or_else(|_| display_path(path))
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn cpu_model() -> String {
    fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| {
                    line.strip_prefix("model name")
                        .or_else(|| line.strip_prefix("Hardware"))
                })
                .and_then(|line| {
                    line.split_once(':')
                        .map(|(_, value)| value.trim().to_owned())
                })
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "<unknown>".to_owned())
}
