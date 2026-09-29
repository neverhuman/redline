//! Bounded engine runs (SQ-09). Every child a case starts runs in its own
//! process group, under a deadline and an output cap, and nothing it leaves
//! behind outlives its run:
//!
//! * stdin is written by its own thread, and stdout and stderr are drained
//!   by reader threads while the child runs, so neither a child that never
//!   reads its input nor one that writes more than a pipe holds can
//!   deadlock the harness;
//! * a reader keeps at most `max_output_bytes` of a stream; the byte after
//!   that kills the group (`output_limit`). A watchdog kills the group at
//!   the deadline (`timeout`);
//! * once the child has exited, whatever is left of its group is killed
//!   too, so a background process it started can neither hold a pipe open
//!   nor survive the case;
//! * a descendant that left the group (a new session) escapes that kill,
//!   so the streams and the stdin writer get only until the case deadline
//!   to finish; a pipe still held open then makes the run a `timeout`.
//!
//! The elapsed time is taken when the child exits, before any of that
//! cleanup, so bounding a run does not change what it measures.
//!
//! The crate takes no libc dependency, so a group is signalled with the
//! `kill` utility (`kill -s KILL -- -<pgid>`); `check_kill` proves it works
//! before a run starts.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::memory::ProcessMemory;

mod files;
mod group;
mod piped;
#[cfg(test)]
mod tests;

pub use files::run_to_files;
pub use piped::run_piped;

pub const DEFAULT_CASE_TIMEOUT_MS: u64 = 60_000;
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// The bounds every engine run of a case is held to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_millis(DEFAULT_CASE_TIMEOUT_MS),
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

impl Limits {
    pub fn new(timeout_ms: u64, max_output_bytes: usize) -> Result<Self> {
        if timeout_ms == 0 {
            bail!("--case-timeout-ms must be positive");
        }
        if max_output_bytes == 0 {
            bail!("--max-output-bytes must be positive");
        }
        Ok(Self {
            timeout: Duration::from_millis(timeout_ms),
            max_output_bytes,
        })
    }

    pub fn timeout_ms(&self) -> u128 {
        self.timeout.as_millis()
    }
}

/// How one engine run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionOutcome {
    /// The child exited on its own with a status code.
    Exited,
    /// The child died of a signal the harness did not send.
    Signal,
    /// The harness killed the child's process group at the deadline.
    Timeout,
    /// The harness killed the child's process group when stdout or stderr
    /// passed the output cap; the kept bytes stop at the cap.
    OutputLimit,
    /// The run could not be prepared, spawned or waited for.
    SpawnError,
    /// Nothing ran: the case was skipped or rejected at selection.
    NotRun,
}

impl ExecutionOutcome {
    /// Whether the run left a whole result the case's contract can judge.
    /// A timed-out, capped or unstarted run did not.
    pub fn is_complete(self) -> bool {
        matches!(self, Self::Exited | Self::Signal)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exited => "exited",
            Self::Signal => "signal",
            Self::Timeout => "timeout",
            Self::OutputLimit => "output_limit",
            Self::SpawnError => "spawn_error",
            Self::NotRun => "not_run",
        }
    }
}

/// What one bounded run left.
#[derive(Debug)]
pub struct Captured {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub outcome: ExecutionOutcome,
    /// From just before the spawn to the child's exit.
    pub elapsed: Duration,
    pub memory: ProcessMemory,
}

/// Fails unless `kill -s 0 -- <this process>` works: without the `kill`
/// utility no case could be bounded, and a run must not start unbounded.
pub fn check_kill() -> Result<()> {
    let status = Command::new("kill")
        .args(["-s", "0", "--", &std::process::id().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("run `kill`, which bounds every case's process group")?;
    if !status.success() {
        bail!("`kill -s 0 -- <own pid>` exited {status}; cases cannot be bounded");
    }
    Ok(())
}

/// The first `cap` bytes of a captured stream.
pub(super) fn read_capped(path: &Path, cap: usize) -> Result<Vec<u8>> {
    let file = fs::File::open(path).with_context(|| format!("read {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(cap as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {}", path.display()))?;
    Ok(bytes)
}
