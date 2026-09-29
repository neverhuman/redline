//! The workload child: the shell fed its scripted transactions, every ack
//! it prints written to the ledger in key order, and the SIGKILL sent at the
//! planned ack count.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::durability_evidence::readback::{self, ACK_PREFIX, MODE_MARKER, READY_MARKER, WORKLOAD};
use crate::durability_evidence::receipt::EvidenceMode;
use crate::recover::harness::describe;
use crate::recover::oracle;

use super::child::{Event, kill, read_lines, spin_for};
use super::{ScenarioPlan, shell};

/// How long the child may take to create its schema and print READY.
const READY_TIMEOUT: Duration = Duration::from_secs(120);
/// Longest gap between two acks before the run counts as stalled.
const ACK_STALL_TIMEOUT: Duration = Duration::from_secs(60);

pub(super) struct WorkloadOutcome {
    pub(super) ready: bool,
    pub(super) mode: Option<String>,
    pub(super) acks_at_kill: usize,
    pub(super) acks_after_kill: usize,
    pub(super) status: String,
    pub(super) kill_result: String,
    pub(super) fault_observed: bool,
}

/// Run the workload child until the planned kill, then drain the acks it
/// had already written. Every ack goes to `ack.ledger` before the next one
/// is read; the ledger is fsynced whenever the pipe is drained, and before
/// the recovery passes.
pub(super) fn drive_workload(
    binary: &Path,
    mode: EvidenceMode,
    plan: &ScenarioPlan,
    dir: &Path,
    db: &Path,
    errors: &mut Vec<String>,
) -> Result<WorkloadOutcome> {
    let script = readback::workload_script(plan.rows)?;
    fs::write(dir.join("workload.sql"), &script)?;
    let stdout_log = File::create(dir.join("workload.stdout"))?;
    let stderr_log = File::create(dir.join("workload.stderr"))?;
    let mut ledger = OpenOptions::new()
        .create_new(true)
        .append(true)
        .open(dir.join("ack.ledger"))
        .context("create ack ledger")?;
    File::open(dir)?.sync_all()?;

    let mut child = shell(binary, mode, db)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr_log))
        .spawn()
        .with_context(|| format!("spawn {}", binary.display()))?;
    let writer = {
        let mut stdin = child.stdin.take().context("child stdin")?;
        thread::spawn(move || stdin.write_all(script.as_bytes()))
    };
    let (events, reader) = read_lines(child.stdout.take().context("child stdout")?, stdout_log);

    let mut outcome = WorkloadOutcome {
        ready: false,
        mode: None,
        acks_at_kill: 0,
        acks_after_kill: 0,
        status: String::new(),
        kill_result: "not sent".to_owned(),
        fault_observed: false,
    };
    let mut tracker = AckTracker {
        plan,
        ledger: &mut ledger,
        outcome: &mut outcome,
        next_key: 0,
        killed: false,
        expect_mode: false,
    };
    let mut last_event = Instant::now();
    'events: loop {
        let first = match events.recv_timeout(Duration::from_millis(200)) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => {
                let limit = if tracker.outcome.ready {
                    ACK_STALL_TIMEOUT
                } else {
                    READY_TIMEOUT
                };
                if !tracker.killed && last_event.elapsed() > limit {
                    errors.push(format!(
                        "no {} within {limit:?}",
                        if tracker.outcome.ready {
                            "ack"
                        } else {
                            "READY"
                        }
                    ));
                    kill(&mut child, tracker.outcome);
                    tracker.killed = true;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        last_event = Instant::now();
        // Take everything the reader has queued, handle it, then fsync the
        // ledger once for the batch.
        let mut batch = vec![first];
        while let Ok(event) = events.try_recv() {
            batch.push(event);
        }
        for event in batch {
            match event {
                Event::Line(line) => tracker.line(&line, &mut child, errors)?,
                Event::Eof => {
                    tracker.ledger.sync_data()?;
                    break 'events;
                }
                Event::Error(err) => {
                    errors.push(format!("read child stdout: {err}"));
                    tracker.ledger.sync_data()?;
                    break 'events;
                }
            }
        }
        tracker.ledger.sync_data()?;
    }
    let (killed, next_key) = (tracker.killed, tracker.next_key);
    ledger.sync_data()?;
    if !killed {
        // The child ended on its own: it finished the script or failed.
        let status = child.wait().context("wait for workload child")?;
        outcome.status = format!("exited before the kill: {}", describe(status));
        errors.push(format!(
            "the workload child {} after {next_key} acks; no kill landed",
            outcome.status
        ));
    }
    let _ = reader.join();
    match writer.join() {
        Ok(Ok(())) => {}
        // The script is written before the child executes it; a broken pipe
        // after the kill is expected.
        Ok(Err(err)) if killed && err.kind() == std::io::ErrorKind::BrokenPipe => {}
        Ok(Err(err)) => errors.push(format!("write workload script: {err}")),
        Err(_) => errors.push("script writer thread panicked".to_owned()),
    }
    Ok(outcome)
}

/// Follows the child's stdout protocol: `@@mode`, the mode, `@@ready`, then
/// `ack|<key>` lines in key order.
struct AckTracker<'a> {
    plan: &'a ScenarioPlan,
    ledger: &'a mut File,
    outcome: &'a mut WorkloadOutcome,
    next_key: u64,
    killed: bool,
    expect_mode: bool,
}

impl AckTracker<'_> {
    fn line(&mut self, line: &str, child: &mut Child, errors: &mut Vec<String>) -> Result<()> {
        if !self.outcome.ready {
            if self.expect_mode {
                self.outcome.mode = Some(line.to_owned());
                self.expect_mode = false;
            } else if line == MODE_MARKER {
                self.expect_mode = true;
            } else if line == READY_MARKER {
                self.outcome.ready = true;
            } else {
                errors.push(format!("unexpected line before READY: {line:?}"));
            }
            return Ok(());
        }
        let Some(key) = line
            .strip_prefix(ACK_PREFIX)
            .and_then(|key| key.parse::<u64>().ok())
        else {
            errors.push(format!("unexpected line after READY: {line:?}"));
            return Ok(());
        };
        if key != self.next_key {
            errors.push(format!(
                "ack for key {key} where {} was next",
                self.next_key
            ));
            return Ok(());
        }
        let digest = oracle::expected_txn_digest(WORKLOAD, key, self.plan.rows);
        self.ledger
            .write_all(oracle::ack_line(key, &digest).as_bytes())?;
        self.next_key += 1;
        if self.killed {
            self.outcome.acks_after_kill += 1;
        } else if self.next_key as usize >= self.plan.kill_after_acks {
            self.outcome.acks_at_kill = self.next_key as usize;
            spin_for(Duration::from_micros(self.plan.kill_delay_us));
            kill(child, self.outcome);
            self.killed = true;
        }
        Ok(())
    }
}
