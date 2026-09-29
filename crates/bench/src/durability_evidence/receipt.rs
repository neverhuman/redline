//! The `durability-evidence.json` receipt: its schema, and the rules a
//! receipt must meet before it may back a public durability claim.
//!
//! A claim is named `durability.<mode>.<failure-model>`, the same string the
//! docs carry in `<!-- claim:durability.<mode>.<failure-model> -->`. Only
//! `process-kill` receipts exist: no tool here produces an OS-crash,
//! power-loss or media-corruption receipt, so a claim on one of those can
//! never be satisfied.

#[path = "receipt/claim.rs"]
mod claim;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::recover::oracle::RecoveryVerdict;

pub use claim::{Claim, receipt_problems, source_problems};

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
