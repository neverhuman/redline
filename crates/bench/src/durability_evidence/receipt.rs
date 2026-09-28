//! The `durability-evidence.json` receipt: its schema, and the rules a
//! receipt must meet before it may back a public durability claim.
//!
//! A claim is named `durability.<mode>.<failure-model>`, the same string the
//! docs carry in `<!-- claim:durability.<mode>.<failure-model> -->`. Only
//! `process-kill` receipts exist: no tool here produces an OS-crash,
//! power-loss or media-corruption receipt, so a claim on one of those can
//! never be satisfied.

use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::recover::oracle::RecoveryVerdict;

pub const SCHEMA_VERSION: u32 = 1;
/// Fewest scenarios a receipt needs before it may back a claim.
pub const CLAIM_MIN_SCENARIOS: usize = 10;
/// Paths whose contents decide what the shipped binary is. A receipt stays
/// valid for a later commit only if none of them changed since its
/// `source_sha`.
pub const BINARY_INPUTS: &[&str] = &[
    "crates",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    ".cargo",
];

/// Commit durability the receipt qualifies. `UnsafeDev` is absent on
/// purpose: it may lose acknowledged commits when the process dies, so there
/// is nothing to qualify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceMode {
    Strict,
    Normal,
}

impl EvidenceMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Normal => "normal",
        }
    }

    /// What `PRAGMA redline_durability` returns in this mode.
    pub fn pragma_value(self) -> &'static str {
        self.as_str()
    }
}

/// The failure the receipt injects. SIGKILL is the only one: it is not a
/// power cut, since the OS keeps every byte the process handed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureModel {
    ProcessKill,
}

impl FailureModel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProcessKill => "process-kill",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryFacts {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    /// First line of `<binary> --version`.
    pub version_line: Option<String>,
    /// ELF machine of the binary (`x86_64`, `aarch64`, ...), if it is ELF.
    pub elf_machine: Option<String>,
    /// The rustc commit embedded in the binary's standard-library paths.
    pub rustc_commit: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailpointScan {
    /// No kernel failpoint name and no `fail` crate string is in the binary.
    pub clean: bool,
    pub markers_checked: usize,
    pub markers_found: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainFacts {
    /// `rustc -V` in the source tree (its `rust-toolchain.toml` applies).
    pub rustc: Option<String>,
    pub rustc_commit: Option<String>,
    /// `host:` of `rustc -vV`, the target triple a plain build produces.
    pub target_triple: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostFacts {
    /// `/proc/sys/kernel/{ostype,osrelease,version}`.
    pub kernel: Option<String>,
    pub arch: String,
    pub logical_cpus: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceFacts {
    pub name: String,
    pub model: Option<String>,
    pub rotational: Option<String>,
    /// `queue/write_cache`: "write back" means the device may hold
    /// acknowledged writes in a volatile cache.
    pub write_cache: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesystemFacts {
    /// Canonical directory the databases were written under.
    pub work_dir: String,
    pub mount_point: String,
    pub fs_type: String,
    pub source: String,
    pub mount_options: String,
    pub super_options: String,
    pub device: Option<DeviceFacts>,
}

impl FilesystemFacts {
    /// tmpfs and ramfs keep files in memory; fsync on them is a no-op.
    pub fn is_memory_backed(&self) -> bool {
        matches!(self.fs_type.as_str(), "tmpfs" | "ramfs")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KillWindow {
    pub min_acks: usize,
    pub max_acks: usize,
    pub max_delay_us: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioCounts {
    pub planned: usize,
    pub run: usize,
    pub passed: usize,
    pub failed: usize,
    /// Runs whose SIGKILL landed on a live child.
    pub faults_observed: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogDigest {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

/// One kill-and-recover scenario.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScenarioRun {
    pub index: usize,
    pub seed: u64,
    /// The harness kills the child once it has read this many acks...
    pub kill_after_acks: usize,
    /// ...and then this many more microseconds have passed.
    pub kill_delay_us: u64,
    pub rows_scripted: usize,
    /// The child printed its READY marker after creating the schema.
    pub ready: bool,
    pub acks_at_kill: usize,
    /// Acks the child had already written to the pipe when it died.
    pub acks_drained_after_kill: usize,
    pub acknowledged: usize,
    pub recovered_acked: usize,
    pub in_flight_committed: Option<u64>,
    pub child_status: String,
    pub kill_result: String,
    pub fault_observed: bool,
    /// `PRAGMA redline_durability` from the workload and each recovery.
    pub effective_modes: Vec<String>,
    pub wal_segments_after_kill: Option<usize>,
    /// `PRAGMA redline_recovery_report` rows, one list per recovery pass.
    pub recovery_reports: Vec<Vec<String>>,
    pub verdict: RecoveryVerdict,
    pub passed: bool,
    pub logs: Vec<LogDigest>,
    /// Kept database and logs of a failed run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Receipt {
    pub schema_version: u32,
    pub tool: String,
    pub created_unix: u64,
    pub source_sha: Option<String>,
    /// The source tree had modified or untracked files when the receipt was
    /// taken, or git could not say.
    pub dirty: bool,
    pub binary: BinaryFacts,
    pub failpoint_scan: FailpointScan,
    pub toolchain: ToolchainFacts,
    pub host: HostFacts,
    pub mode: EvidenceMode,
    /// `PRAGMA redline_durability` as every child reported it, when they
    /// all agreed.
    pub effective_mode: Option<String>,
    pub failure_model: FailureModel,
    pub filesystem: Option<FilesystemFacts>,
    /// Environment variables the children received; everything else is
    /// cleared.
    pub child_environment: Vec<String>,
    pub oracle: String,
    pub seed: u64,
    pub kill_window: KillWindow,
    pub scenarios: ScenarioCounts,
    /// Reasons the tool refused to run any scenario.
    pub rejections: Vec<String>,
    pub runs: Vec<ScenarioRun>,
    pub passed: bool,
    pub limitations: Vec<String>,
    pub logs_dir: Option<String>,
}

/// `durability.<mode>.<failure-model>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub text: String,
    pub mode: String,
    pub failure_model: String,
}

impl Claim {
    pub fn parse(text: &str) -> Result<Self> {
        let parts: Vec<&str> = text.split('.').collect();
        let [prefix, mode, failure_model] = parts.as_slice() else {
            bail!("claim {text:?} is not durability.<mode>.<failure-model>");
        };
        if *prefix != "durability" {
            bail!("claim {text:?} does not start with `durability.`");
        }
        if !matches!(*mode, "strict" | "normal" | "unsafe-dev") {
            bail!("claim {text:?} names mode {mode:?}; expected strict, normal or unsafe-dev");
        }
        if !matches!(
            *failure_model,
            "process-kill" | "power-loss" | "media-corruption"
        ) {
            bail!(
                "claim {text:?} names failure model {failure_model:?}; expected process-kill, power-loss or media-corruption"
            );
        }
        Ok(Self {
            text: text.to_owned(),
            mode: (*mode).to_owned(),
            failure_model: (*failure_model).to_owned(),
        })
    }

    pub fn covers(&self, receipt: &Receipt) -> bool {
        receipt.mode.as_str() == self.mode && receipt.failure_model.as_str() == self.failure_model
    }
}

/// Every reason `receipt` cannot back a claim. Empty means it can.
pub fn receipt_problems(receipt: &Receipt) -> Vec<String> {
    let mut problems = Vec::new();
    if receipt.schema_version != SCHEMA_VERSION {
        problems.push(format!(
            "schema_version {} is not {SCHEMA_VERSION}",
            receipt.schema_version
        ));
    }
    if !receipt.passed {
        problems.push("the receipt says passed=false".to_owned());
    }
    for rejection in &receipt.rejections {
        problems.push(format!("the tool rejected the run: {rejection}"));
    }
    let counts = &receipt.scenarios;
    if counts.planned < CLAIM_MIN_SCENARIOS {
        problems.push(format!(
            "{} scenarios planned; a claim needs at least {CLAIM_MIN_SCENARIOS}",
            counts.planned
        ));
    }
    if counts.run != counts.planned || counts.passed != counts.planned || counts.failed != 0 {
        problems.push(format!(
            "scenarios planned {}, run {}, passed {}, failed {}",
            counts.planned, counts.run, counts.passed, counts.failed
        ));
    }
    if counts.faults_observed != counts.run {
        problems.push(format!(
            "the kill landed on a live process in {} of {} runs",
            counts.faults_observed, counts.run
        ));
    }
    if receipt.runs.len() != counts.run {
        problems.push(format!(
            "{} runs listed but scenarios.run is {}",
            receipt.runs.len(),
            counts.run
        ));
    }
    for run in &receipt.runs {
        if !run.passed || !run.verdict.qualified {
            problems.push(format!(
                "scenario {} failed: {}",
                run.index,
                run.verdict.summary()
            ));
        }
        if !run.fault_observed {
            problems.push(format!("scenario {}: no kill landed", run.index));
        }
        if run.acknowledged == 0 {
            problems.push(format!("scenario {}: nothing was acknowledged", run.index));
        }
    }
    if !receipt.failpoint_scan.clean || receipt.failpoint_scan.markers_checked == 0 {
        problems.push(format!(
            "the binary is a failpoint build or was not scanned (found {:?})",
            receipt.failpoint_scan.markers_found
        ));
    }
    if receipt.dirty {
        problems.push("the source tree was dirty".to_owned());
    }
    match receipt.source_sha.as_deref() {
        Some(sha) if is_hex(sha, 40) => {}
        other => problems.push(format!("source_sha {other:?} is not a commit id")),
    }
    if !is_hex(&receipt.binary.sha256, 64) {
        problems.push(format!(
            "binary.sha256 {:?} is not a SHA-256",
            receipt.binary.sha256
        ));
    }
    if receipt.effective_mode.as_deref() != Some(receipt.mode.pragma_value()) {
        problems.push(format!(
            "effective mode {:?} is not the requested {}",
            receipt.effective_mode,
            receipt.mode.as_str()
        ));
    }
    match &receipt.filesystem {
        None => problems.push("the receipt names no filesystem".to_owned()),
        Some(fs) if receipt.mode == EvidenceMode::Strict && fs.is_memory_backed() => {
            problems.push(format!(
                "a Strict receipt on {} ({}) proves nothing about fsync",
                fs.fs_type, fs.mount_point
            ));
        }
        Some(_) => {}
    }
    problems
}

/// Check that `source_sha` is `at` or an ancestor of it, and that no path
/// in [`BINARY_INPUTS`] changed between them.
pub fn source_problems(repo: &Path, source_sha: &str, at: &str) -> Vec<String> {
    let git = |args: &[&str]| Command::new("git").arg("-C").arg(repo).args(args).output();
    let mut problems = Vec::new();
    match git(&["cat-file", "-e", &format!("{source_sha}^{{commit}}")]) {
        Ok(out) if out.status.success() => {}
        Ok(_) => {
            problems.push(format!(
                "source_sha {source_sha} is not a commit in {}",
                repo.display()
            ));
            return problems;
        }
        Err(err) => {
            problems.push(format!("run git in {}: {err}", repo.display()));
            return problems;
        }
    }
    match git(&["merge-base", "--is-ancestor", source_sha, at]) {
        Ok(out) if out.status.success() => {}
        Ok(_) => problems.push(format!(
            "source_sha {source_sha} is not an ancestor of {at}"
        )),
        Err(err) => problems.push(format!("git merge-base: {err}")),
    }
    let mut diff = vec!["diff", "--name-only", source_sha, at, "--"];
    diff.extend(BINARY_INPUTS);
    match git(&diff) {
        Ok(out) if out.status.success() => {
            let changed: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::to_owned)
                .collect();
            if !changed.is_empty() {
                problems.push(format!(
                    "{} binary input files changed between source_sha {source_sha} and {at}, first {:?}",
                    changed.len(),
                    &changed[..changed.len().min(8)]
                ));
            }
        }
        Ok(out) => problems.push(format!(
            "git diff {source_sha} {at}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(err) => problems.push(format!("git diff: {err}")),
    }
    problems
}

fn is_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| b.is_ascii_hexdigit())
}
