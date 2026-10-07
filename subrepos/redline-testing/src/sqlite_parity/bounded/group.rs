//! A child's process group and the one kill the harness may send it, how
//! the child is spawned into that group, and how its stdin is fed and
//! waited for.

use std::io::Write;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::{ExecutionOutcome, Limits};

/// The least time the streams and the stdin writer get to finish once the
/// child has exited, even when it exited just before the case deadline.
const DRAIN_GRACE: Duration = Duration::from_millis(200);

const NOT_FIRED: u8 = 0;
pub(super) const FIRED_TIMEOUT: u8 = 1;
pub(super) const FIRED_OUTPUT_LIMIT: u8 = 2;

/// A child's process group (its pgid is the child's pid) and the one kill
/// the harness may send it before the child exits.
pub(super) struct Group {
    pgid: u32,
    fired: AtomicU8,
}

impl Group {
    pub(super) fn new(pgid: u32) -> Self {
        Self {
            pgid,
            fired: AtomicU8::new(NOT_FIRED),
        }
    }

    /// Kills the group, and its leader in case it left the group, for
    /// `reason`. Only the first reason counts.
    pub(super) fn fire(&self, reason: u8) {
        if self
            .fired
            .compare_exchange(NOT_FIRED, reason, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            signal_kill(&[format!("-{}", self.pgid), self.pgid.to_string()]);
        }
    }

    /// Records `reason` without signalling: the leader was already reaped,
    /// so its pid may name another process by now. Only the first reason
    /// counts.
    pub(super) fn mark(&self, reason: u8) {
        let _ = self
            .fired
            .compare_exchange(NOT_FIRED, reason, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// After the leader exited: kills whatever is left of its group. The
    /// leader was reaped, so a group that is already empty makes `kill`
    /// report "no such process", which is the expected case.
    pub(super) fn sweep(&self) {
        signal_kill(&[format!("-{}", self.pgid)]);
    }

    pub(super) fn outcome(&self, status: ExitStatus) -> ExecutionOutcome {
        match self.fired.load(Ordering::SeqCst) {
            FIRED_TIMEOUT => super::deadline_classification::expired(status),
            FIRED_OUTPUT_LIMIT => ExecutionOutcome::OutputLimit,
            _ if status.code().is_some() => ExecutionOutcome::Exited,
            _ => ExecutionOutcome::Signal,
        }
    }
}

fn signal_kill(targets: &[String]) {
    let _ = Command::new("kill")
        .args(["-s", "KILL", "--"])
        .args(targets)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub(super) fn stdin_mode(stdin: &Option<Vec<u8>>) -> Stdio {
    if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    }
}

pub(super) fn spawn_in_group(command: &mut Command) -> Result<Child> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn().context("spawn sqlite parity child")
}

/// When the streams and the stdin writer must have finished: the case
/// deadline, or `DRAIN_GRACE` from now if that is later.
pub(super) fn drain_deadline(started: Instant, limits: Limits) -> Instant {
    (started + limits.timeout).max(Instant::now() + DRAIN_GRACE)
}

/// Writes all of `stdin` and closes the pipe. A child that exits or is
/// killed without reading it ends the write with a broken pipe, which is
/// not an error of the run. The receiver hears once the write is over.
pub(super) fn spawn_writer(
    child: &mut Child,
    stdin: Option<Vec<u8>>,
) -> Option<mpsc::Receiver<()>> {
    let bytes = stdin?;
    let mut pipe = child.stdin.take()?;
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let _ = pipe.write_all(&bytes);
        drop(pipe);
        let _ = done.send(());
    });
    Some(finished)
}

/// Whether the stdin writer finished by `deadline`. A writer still blocked
/// then is left behind: nothing in the group reads its pipe any more.
pub(super) fn finished_by(writer: &mpsc::Receiver<()>, deadline: Instant) -> bool {
    writer
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .is_ok()
}

#[cfg(all(test, unix))]
#[path = "deadline_classification_tests.rs"]
mod deadline_classification_tests;
