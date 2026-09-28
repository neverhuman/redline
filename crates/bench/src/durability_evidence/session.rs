//! One scenario: run the shipped shell on the workload, SIGKILL it at the
//! planned point, then recover twice with the same binary and grade the
//! result with the recover oracle.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::recover::harness::describe;
use crate::recover::oracle::{self, AckLedger};

use super::preflight::sha256_hex;
use super::readback::{self, ACK_PREFIX, MODE_MARKER, READY_MARKER, WORKLOAD};
use super::receipt::{EvidenceMode, LogDigest, ScenarioRun};

/// How long the child may take to create its schema and print READY.
const READY_TIMEOUT: Duration = Duration::from_secs(120);
/// Longest gap between two acks before the run counts as stalled.
const ACK_STALL_TIMEOUT: Duration = Duration::from_secs(60);
/// How long one recovery pass may run.
const PASS_TIMEOUT: Duration = Duration::from_secs(300);

/// `REDLINEDB_DEFAULT_DURABILITY`, as the `redlinedb` crate spells it.
const DURABILITY_ENV: &str = "REDLINEDB_DEFAULT_DURABILITY";
const QUIET_DURABILITY_ENV: &str = "REDLINEDB_QUIET_DURABILITY";

#[derive(Debug, Clone)]
pub struct ScenarioPlan {
    pub index: usize,
    pub seed: u64,
    pub kill_after_acks: usize,
    pub kill_delay_us: u64,
    pub rows: usize,
}

/// The environment every child gets: cleared, then `PATH` and, for Normal,
/// the durability variable. Strict is the default the binary ships with.
pub fn child_environment(mode: EvidenceMode) -> Vec<(String, String)> {
    let mut env = Vec::new();
    if let Ok(path) = std::env::var("PATH") {
        env.push(("PATH".to_owned(), path));
    }
    if mode == EvidenceMode::Normal {
        env.push((DURABILITY_ENV.to_owned(), "normal".to_owned()));
        env.push((QUIET_DURABILITY_ENV.to_owned(), "1".to_owned()));
    }
    env
}

fn shell(binary: &Path, mode: EvidenceMode, db: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .env_clear()
        .envs(child_environment(mode))
        .arg("-batch")
        .arg("-bail")
        .arg(db);
    command
}

pub fn run_scenario(
    binary: &Path,
    mode: EvidenceMode,
    plan: &ScenarioPlan,
    dir: &Path,
) -> Result<ScenarioRun> {
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let db = dir.join("db");
    let mut run = ScenarioRun {
        index: plan.index,
        seed: plan.seed,
        kill_after_acks: plan.kill_after_acks,
        kill_delay_us: plan.kill_delay_us,
        rows_scripted: plan.rows,
        ..ScenarioRun::default()
    };
    let mut errors = Vec::new();
    let workload = drive_workload(binary, mode, plan, dir, &db, &mut errors)?;
    run.ready = workload.ready;
    run.acks_at_kill = workload.acks_at_kill;
    run.acks_drained_after_kill = workload.acks_after_kill;
    run.child_status = workload.status;
    run.kill_result = workload.kill_result;
    run.fault_observed = workload.fault_observed;
    run.effective_modes
        .push(workload.mode.clone().unwrap_or_default());
    run.wal_segments_after_kill = count_segments(&db.join("wal"));

    let ledger_text = fs::read_to_string(dir.join("ack.ledger")).unwrap_or_default();
    let ledger = match AckLedger::parse(WORKLOAD, plan.rows, true, &ledger_text) {
        Ok(ledger) => ledger,
        Err(err) => {
            errors.push(format!("ack ledger: {err:#}"));
            AckLedger::new(WORKLOAD, plan.rows, true)
        }
    };
    if ledger.acked.is_empty() {
        errors.push("no transaction was acknowledged, so the run proves nothing".to_owned());
    }

    let first = recovery_pass(binary, mode, &db, dir, 1);
    let second = recovery_pass(binary, mode, &db, dir, 2);
    let mut observed = first.state.clone();
    for pass in [&first, &second] {
        errors.extend(pass.errors.iter().cloned());
        run.effective_modes
            .push(pass.mode.clone().unwrap_or_default());
        run.recovery_reports.push(pass.report.clone());
    }
    errors.extend(first.state.image_differences(&second.state));
    for (label, found) in ["workload", "recovery 1", "recovery 2"]
        .iter()
        .zip(&run.effective_modes)
    {
        if found != mode.pragma_value() {
            errors.push(format!(
                "{label}: PRAGMA redline_durability said {found:?}, not {}",
                mode.pragma_value()
            ));
        }
    }
    observed.child_started = workload.ready;
    observed.fault_observed = workload.fault_observed;
    observed.harness_errors = errors;
    let verdict = oracle::evaluate(&ledger, &observed);
    run.acknowledged = verdict.acknowledged;
    run.recovered_acked = verdict.recovered_acked;
    run.in_flight_committed = verdict.in_flight_committed;
    run.passed = verdict.qualified;
    run.verdict = verdict;
    run.logs = digest_logs(dir)?;
    if run.passed {
        fs::remove_dir_all(&db).with_context(|| format!("remove {}", db.display()))?;
    } else {
        run.evidence_dir = Some(dir.display().to_string());
    }
    Ok(run)
}

struct WorkloadOutcome {
    ready: bool,
    mode: Option<String>,
    acks_at_kill: usize,
    acks_after_kill: usize,
    status: String,
    kill_result: String,
    fault_observed: bool,
}

enum Event {
    Line(String),
    Eof,
    Error(String),
}

/// Run the workload child until the planned kill, then drain the acks it
/// had already written. Every ack goes to `ack.ledger` before the next one
/// is read; the ledger is fsynced whenever the pipe is drained, and before
/// the recovery passes.
fn drive_workload(
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

/// SIGKILL the child, then reap it. The fault counts only when the signal
/// reached a live child and the child died of it.
fn kill(child: &mut Child, outcome: &mut WorkloadOutcome) {
    match child.try_wait() {
        Ok(Some(status)) => {
            outcome.status = format!("exited before the kill: {}", describe(status));
            outcome.kill_result = "not sent".to_owned();
            return;
        }
        Ok(None) => {}
        Err(err) => {
            outcome.status = format!("try_wait failed: {err}");
            return;
        }
    }
    outcome.kill_result = match child.kill() {
        Ok(()) => "sent SIGKILL".to_owned(),
        Err(err) => format!("kill failed: {err}"),
    };
    match child.wait() {
        Ok(status) => {
            outcome.fault_observed =
                outcome.kill_result == "sent SIGKILL" && died_of_sigkill(status);
            outcome.status = describe(status);
        }
        Err(err) => outcome.status = format!("wait failed: {err}"),
    }
}

#[cfg(unix)]
fn died_of_sigkill(status: ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt;
    status.signal() == Some(libc::SIGKILL)
}

#[cfg(not(unix))]
fn died_of_sigkill(_status: ExitStatus) -> bool {
    false
}

/// Busy-wait: `thread::sleep` has millisecond-scale slack, and the delay
/// is what moves the kill around inside one commit.
fn spin_for(delay: Duration) {
    let until = Instant::now() + delay;
    while Instant::now() < until {
        std::hint::spin_loop();
    }
}

/// Copy the child's stdout byte for byte to `log` and hand every line to
/// the returned receiver.
fn read_lines(stdout: ChildStdout, mut log: File) -> (Receiver<Event>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => {
                    let _ = tx.send(Event::Eof);
                    break;
                }
                Ok(_) => {
                    if let Err(err) = log.write_all(&buf) {
                        let _ = tx.send(Event::Error(format!("write stdout log: {err}")));
                        break;
                    }
                    if buf.last() != Some(&b'\n') {
                        // A partial last line: the child died mid-write. It
                        // is logged, and it is not an ack.
                        let _ = tx.send(Event::Eof);
                        break;
                    }
                    let line = String::from_utf8_lossy(&buf[..buf.len() - 1]).into_owned();
                    if tx.send(Event::Line(line)).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    let _ = tx.send(Event::Error(err.to_string()));
                    break;
                }
            }
        }
        let _ = log.sync_all();
    });
    (rx, handle)
}

/// Open the database with the shipped shell (which runs crash recovery),
/// read everything the oracle needs, and exit.
fn recovery_pass(
    binary: &Path,
    mode: EvidenceMode,
    db: &Path,
    dir: &Path,
    pass: usize,
) -> readback::PassObservation {
    let stdout_path = dir.join(format!("recover-{pass}.stdout"));
    let stderr_path = dir.join(format!("recover-{pass}.stderr"));
    match run_pass(binary, mode, db, &stdout_path, &stderr_path) {
        Ok(status) => {
            let stdout = fs::read_to_string(&stdout_path).unwrap_or_default();
            let mut observation = readback::parse_readback(&stdout);
            if !status.success() {
                observation.errors.push(format!(
                    "recovery pass {pass} exited {}: {}",
                    describe(status),
                    stderr_tail(&stderr_path)
                ));
            }
            for error in &mut observation.errors {
                *error = format!("recovery pass {pass}: {error}");
            }
            observation
        }
        Err(err) => readback::PassObservation {
            errors: vec![format!("recovery pass {pass}: {err:#}")],
            ..readback::PassObservation::default()
        },
    }
}

fn run_pass(
    binary: &Path,
    mode: EvidenceMode,
    db: &Path,
    stdout: &Path,
    stderr: &Path,
) -> Result<ExitStatus> {
    if !db.is_dir() {
        anyhow::bail!("database {} does not exist", db.display());
    }
    let mut child = shell(binary, mode, db)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(File::create(stdout)?))
        .stderr(Stdio::from(File::create(stderr)?))
        .spawn()
        .with_context(|| format!("spawn {}", binary.display()))?;
    {
        let mut stdin = child.stdin.take().context("child stdin")?;
        stdin.write_all(readback::readback_script().as_bytes())?;
    }
    let deadline = Instant::now() + PASS_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("did not finish within {PASS_TIMEOUT:?}");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn stderr_tail(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(8)..].join(" / ")
}

fn count_segments(wal: &Path) -> Option<usize> {
    let entries = fs::read_dir(wal).ok()?;
    Some(
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "wal"))
            .count(),
    )
}

const LOGS: &[&str] = &[
    "workload.sql",
    "workload.stdout",
    "workload.stderr",
    "ack.ledger",
    "recover-1.stdout",
    "recover-1.stderr",
    "recover-2.stdout",
    "recover-2.stderr",
];

fn digest_logs(dir: &Path) -> Result<Vec<LogDigest>> {
    let mut out = Vec::new();
    for name in LOGS {
        let path: PathBuf = dir.join(name);
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        out.push(LogDigest {
            name: (*name).to_owned(),
            bytes: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
        });
    }
    Ok(out)
}
