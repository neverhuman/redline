use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::bounded::ExecutionOutcome;
use super::case::{Case, ComparisonMode};
use super::compare::NORMALIZATION_POLICY;
use super::engine::EngineOutput;
use super::order::{FirstEngine, MeasurementOrder};
use super::runner::{Verdict, VerdictReason, VerdictStage};

#[derive(Debug, Serialize)]
pub struct CompareRecord {
    pub case_id: String,
    pub name: String,
    pub case_file: String,
    pub priority: String,
    pub profile: String,
    pub category: String,
    pub sample_index: usize,
    pub repetition_index: Option<usize>,
    pub sample_role: String,
    /// The run's engine order (BM3-04), on every record.
    pub measurement_order: MeasurementOrder,
    /// The engine this sample ran first; `null` on a placeholder of a case
    /// that did not run.
    pub first_engine: Option<FirstEngine>,
    pub sqlite_version: Option<String>,
    pub reference_engine: String,
    pub target_engine: String,
    pub reference_executable_path: String,
    pub target_executable_path: String,
    pub reference_executable_sha256: String,
    pub target_executable_sha256: String,
    pub reference_version: String,
    pub target_version: String,
    pub status: String,
    pub verdict_reason: VerdictReason,
    pub stage: VerdictStage,
    /// How the sample's engine runs ended (SQ-09): the first that did not
    /// exit on its own, the reference's before the target's, else `exited`.
    pub execution_outcome: ExecutionOutcome,
    pub reference_execution_outcome: ExecutionOutcome,
    pub target_execution_outcome: ExecutionOutcome,
    /// The `scope-policy.json` exception a skipped case was skipped under
    /// (SQ-05); `null` for every other record.
    pub policy_exception_id: Option<String>,
    /// The comparison rules this record was judged by (SQ-06).
    pub normalization_policy: &'static str,
    pub comparison_mode: ComparisonMode,
    pub reference_exit_code: Option<i32>,
    pub target_exit_code: Option<i32>,
    pub reference_elapsed_ns: u128,
    pub target_elapsed_ns: u128,
    pub latency_ratio: f64,
    pub reference_stdout_sha256: String,
    pub reference_stderr_sha256: String,
    pub target_stdout_sha256: String,
    pub target_stderr_sha256: String,
    pub artifact_dir: Option<PathBuf>,
    pub diagnostic: Option<String>,
    pub memory_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_peak_rss_kb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_rss_sampled_kb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_peak_rss_kb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_rss_sampled_kb: Option<u64>,
}

/// What every record of one suite run names: the two engines, the
/// reference's version and the run's engine order.
pub struct RunLabels<'a> {
    pub reference_engine: &'a str,
    pub target_engine: &'a str,
    pub sqlite_version: Option<String>,
    pub measurement_order: MeasurementOrder,
}

/// An executed sample's place in its case and the engine it ran first.
pub struct SamplePosition {
    pub sample_index: usize,
    pub repetition_index: Option<usize>,
    pub sample_role: String,
    pub first_engine: FirstEngine,
}

pub fn skipped_compare_record(
    case: &Case,
    labels: &RunLabels<'_>,
    artifact_dir: Option<PathBuf>,
    diagnostic: Option<String>,
    policy_exception_id: &str,
) -> CompareRecord {
    CompareRecord {
        case_id: case.display_id(),
        name: case.name.clone(),
        case_file: case.case_file_name(),
        priority: case.priority.to_string(),
        profile: case.profile.to_string(),
        category: case.category.clone(),
        sample_index: 0,
        repetition_index: None,
        sample_role: "skipped".to_owned(),
        measurement_order: labels.measurement_order,
        first_engine: None,
        sqlite_version: labels.sqlite_version.clone(),
        reference_engine: labels.reference_engine.to_owned(),
        target_engine: labels.target_engine.to_owned(),
        reference_executable_path: String::new(),
        target_executable_path: String::new(),
        reference_executable_sha256: String::new(),
        target_executable_sha256: String::new(),
        reference_version: String::new(),
        target_version: String::new(),
        status: "skipped".to_owned(),
        verdict_reason: VerdictReason::Skipped,
        stage: VerdictStage::Selection,
        execution_outcome: ExecutionOutcome::NotRun,
        reference_execution_outcome: ExecutionOutcome::NotRun,
        target_execution_outcome: ExecutionOutcome::NotRun,
        policy_exception_id: Some(policy_exception_id.to_owned()),
        normalization_policy: NORMALIZATION_POLICY,
        comparison_mode: case.comparison_mode,
        reference_exit_code: None,
        target_exit_code: None,
        reference_elapsed_ns: 0,
        target_elapsed_ns: 0,
        latency_ratio: 0.0,
        reference_stdout_sha256: String::new(),
        reference_stderr_sha256: String::new(),
        target_stdout_sha256: String::new(),
        target_stderr_sha256: String::new(),
        artifact_dir,
        diagnostic,
        memory_status: "not_run".to_owned(),
        reference_peak_rss_kb: None,
        reference_rss_sampled_kb: None,
        target_peak_rss_kb: None,
        target_rss_sampled_kb: None,
    }
}

/// The one record of a case that failed at selection (SQ-05): nothing ran,
/// so it is a `not_run` placeholder whose status is `failed`.
pub fn selection_failure_record(
    case: &Case,
    labels: &RunLabels<'_>,
    artifact_dir: PathBuf,
    verdict: &Verdict,
) -> CompareRecord {
    let mut record = skipped_compare_record(
        case,
        labels,
        Some(artifact_dir),
        verdict.diagnostic.clone(),
        "",
    );
    record.sample_role = "not_run".to_owned();
    record.status = verdict.status().to_owned();
    record.verdict_reason = verdict.reason;
    record.stage = verdict.stage;
    record.policy_exception_id = None;
    record
}

pub fn compare_record(
    case: &Case,
    reference_output: &EngineOutput,
    target_output: &EngineOutput,
    labels: &RunLabels<'_>,
    position: SamplePosition,
    verdict: &Verdict,
    artifact_dir: Option<PathBuf>,
) -> CompareRecord {
    let reference_ns = reference_output.elapsed.as_nanos().max(1) as f64;
    let ratio = target_output.elapsed.as_nanos() as f64 / reference_ns;
    CompareRecord {
        case_id: case.display_id(),
        name: case.name.clone(),
        case_file: case.case_file_name(),
        priority: case.priority.to_string(),
        profile: case.profile.to_string(),
        category: case.category.clone(),
        sample_index: position.sample_index,
        repetition_index: position.repetition_index,
        sample_role: position.sample_role,
        measurement_order: labels.measurement_order,
        first_engine: Some(position.first_engine),
        sqlite_version: labels.sqlite_version.clone(),
        reference_engine: reference_output.engine.clone(),
        target_engine: target_output.engine.clone(),
        reference_executable_path: reference_output.executable_path.clone(),
        target_executable_path: target_output.executable_path.clone(),
        reference_executable_sha256: reference_output.executable_sha256.clone(),
        target_executable_sha256: target_output.executable_sha256.clone(),
        reference_version: reference_output.version.clone(),
        target_version: target_output.version.clone(),
        status: verdict.status().to_owned(),
        verdict_reason: verdict.reason,
        stage: verdict.stage,
        execution_outcome: [reference_output.outcome, target_output.outcome]
            .into_iter()
            .find(|outcome| *outcome != ExecutionOutcome::Exited)
            .unwrap_or(ExecutionOutcome::Exited),
        reference_execution_outcome: reference_output.outcome,
        target_execution_outcome: target_output.outcome,
        policy_exception_id: None,
        normalization_policy: NORMALIZATION_POLICY,
        comparison_mode: case.comparison_mode,
        reference_exit_code: reference_output.status_code,
        target_exit_code: target_output.status_code,
        reference_elapsed_ns: reference_output.elapsed.as_nanos(),
        target_elapsed_ns: target_output.elapsed.as_nanos(),
        latency_ratio: ratio,
        reference_stdout_sha256: sha256_hex(&reference_output.stdout),
        reference_stderr_sha256: sha256_hex(&reference_output.stderr),
        target_stdout_sha256: sha256_hex(&target_output.stdout),
        target_stderr_sha256: sha256_hex(&target_output.stderr),
        artifact_dir,
        diagnostic: verdict.diagnostic.clone(),
        memory_status: merge_memory_status(reference_output, target_output),
        reference_peak_rss_kb: reference_output.peak_rss_kb,
        reference_rss_sampled_kb: reference_output.rss_sampled_kb,
        target_peak_rss_kb: target_output.peak_rss_kb,
        target_rss_sampled_kb: target_output.rss_sampled_kb,
    }
}

fn merge_memory_status(reference_output: &EngineOutput, target_output: &EngineOutput) -> String {
    if reference_output.memory_status == "sampled" || target_output.memory_status == "sampled" {
        "sampled".to_owned()
    } else if reference_output.memory_status == "disabled"
        && target_output.memory_status == "disabled"
    {
        "disabled".to_owned()
    } else {
        "unavailable".to_owned()
    }
}

pub fn write_failure_artifact(
    case: &Case,
    outputs: &[&EngineOutput],
    reason: &str,
) -> Result<PathBuf> {
    let root = Path::new("target")
        .join("sqlite-parity")
        .join("failures")
        .join(format!("{}_{}", case.display_id(), std::process::id()));
    fs::create_dir_all(&root).with_context(|| format!("create {}", root.display()))?;
    fs::write(root.join("input.sql"), &case.stdin)?;
    fs::write(root.join("reason.txt"), reason)?;
    for output in outputs {
        let prefix = output
            .engine
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .collect::<String>();
        fs::write(root.join(format!("{prefix}.stdout.txt")), &output.stdout)?;
        fs::write(root.join(format!("{prefix}.stderr.txt")), &output.stderr)?;
        fs::write(
            root.join(format!("{prefix}.exit.txt")),
            format!("{:?}\n", output.status_code),
        )?;
    }
    Ok(root)
}

pub fn write_skip_artifact(case: &Case, reason: &str) -> Result<PathBuf> {
    let root = Path::new("target")
        .join("sqlite-parity")
        .join("skips")
        .join(format!("{}_{}", case.display_id(), std::process::id()));
    fs::create_dir_all(&root).with_context(|| format!("create {}", root.display()))?;
    fs::write(root.join("input.sql"), &case.stdin)?;
    fs::write(root.join("reason.txt"), reason)?;
    Ok(root)
}

/// SHA-256 of exactly the bytes a shell wrote.
fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
