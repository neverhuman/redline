//! Parent side of a crash-recovery run: spawn the child, wait for READY,
//! kill it mid-workload, then recover twice and grade the result with the
//! oracle.

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::{DurabilityKind, EngineKind, RecoverChildArgs, RecoveryScenarioKind};
use crate::engine::engine_name;

use super::observe;
use super::oracle::{self, AckLedger, RecoveredState, RecoveryVerdict, Workload};

/// The line a child prints on stdout once its schema is set up and its
/// ack log exists. The kill timer starts only after this line.
pub const READY_LINE: &str = "READY";
/// How long the parent waits for READY before calling the run failed.
const READY_TIMEOUT: Duration = Duration::from_secs(120);
/// Lines of child stderr kept in a failure report.
const STDERR_TAIL_LINES: usize = 40;

/// Settings shared by every run of one invocation.
#[derive(Debug, Clone)]
pub struct HarnessContext {
    pub child_exe: PathBuf,
    pub seed: u64,
    pub git_sha: Option<String>,
}

impl HarnessContext {
    pub fn new(child_exe: Option<&Path>, seed: u64) -> Result<Self> {
        let child_exe = match child_exe {
            Some(path) => path.to_path_buf(),
            None => std::env::current_exe().context("resolve current executable")?,
        };
        Ok(Self {
            child_exe,
            seed,
            git_sha: crate::report::collect_environment().git_sha,
        })
    }
}

/// Where a failed run left its files, so it can be inspected or replayed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureEvidence {
    pub dir: PathBuf,
    pub seed: u64,
    pub git_sha: Option<String>,
    pub stderr_tail: String,
}

/// Result of one run, shared by `recover` and `recover-matrix`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseOutcome {
    pub acknowledged: usize,
    /// Acknowledged transactions recovered with their exact contents.
    pub recovered: usize,
    pub child_status: String,
    pub kill_result: String,
    pub verdict: RecoveryVerdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<FailureEvidence>,
}

pub struct CaseSpec<'a> {
    pub label: &'a str,
    pub engine: EngineKind,
    pub durability: DurabilityKind,
    pub scenario: RecoveryScenarioKind,
    pub rows: usize,
    pub checkpoint_every_rows: usize,
    pub kill_after: Duration,
}

pub fn run_case(ctx: &HarnessContext, spec: &CaseSpec<'_>) -> Result<CaseOutcome> {
    let tmp = tempfile::Builder::new()
        .prefix("redline-recover-")
        .tempdir()
        .with_context(|| format!("create recovery tempdir for {}", spec.label))?;
    let stem = format!("{:?}-{}", spec.engine, spec.label);
    let db_dir = tmp.path().join(&stem);
    let ack_log = tmp.path().join(format!("{stem}.ack"));
    let stderr_path = tmp.path().join("child.stderr");
    let rows = spec.rows.max(1);
    let child_args = RecoverChildArgs {
        engine: spec.engine,
        durability: spec.durability,
        scenario: spec.scenario,
        db_dir: db_dir.clone(),
        ack_log: ack_log.clone(),
        rows,
        checkpoint_every_rows: spec.checkpoint_every_rows.max(1),
    };
    let mut child = spawn_recovery_child(&ctx.child_exe, &child_args, &stderr_path)?;
    let ready = wait_ready(&mut child, READY_TIMEOUT);
    let stop = if ready {
        thread::sleep(spec.kill_after);
        stop_child(&mut child)
    } else {
        let mut stop = stop_child(&mut child);
        stop.status = format!("no READY line: {}", stop.status);
        stop.fault_observed = false;
        stop
    };

    let workload = Workload::from_scenario(spec.scenario);
    let mut harness_errors = Vec::new();
    let ledger = read_ledger(&ack_log, workload, rows, true, ready, &mut harness_errors);
    let mut observed = recover_twice(spec, &db_dir, &mut harness_errors);
    observed.child_started = ready;
    observed.fault_observed = stop.fault_observed;
    observed.harness_errors = harness_errors;
    let verdict = oracle::evaluate(&ledger, &observed);

    let evidence = if verdict.qualified {
        None
    } else {
        Some(keep_evidence(tmp, ctx, "child.stderr"))
    };
    Ok(CaseOutcome {
        acknowledged: verdict.acknowledged,
        recovered: verdict.recovered_acked,
        child_status: stop.status,
        kill_result: stop.kill_result,
        verdict,
        evidence,
    })
}

/// Parse the ack ledger. A missing ledger after READY is a failure: the
/// children create it before printing READY.
pub(crate) fn read_ledger(
    ack_log: &Path,
    workload: Workload,
    rows: usize,
    expect_fault: bool,
    ready: bool,
    harness_errors: &mut Vec<String>,
) -> AckLedger {
    let empty = AckLedger::new(workload, rows, expect_fault);
    match fs::read_to_string(ack_log) {
        Ok(text) => match AckLedger::parse(workload, rows, expect_fault, &text) {
            Ok(ledger) => ledger,
            Err(err) => {
                harness_errors.push(format!("ack ledger {}: {err:#}", ack_log.display()));
                empty
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if ready {
                harness_errors.push(format!(
                    "ack ledger {} missing although the child reported READY",
                    ack_log.display()
                ));
            }
            empty
        }
        Err(err) => {
            harness_errors.push(format!("read ack ledger {}: {err}", ack_log.display()));
            empty
        }
    }
}

/// Recover the database, read it, close it, then recover and read it again.
/// The second pass must reproduce the first exactly.
fn recover_twice(
    spec: &CaseSpec<'_>,
    db_dir: &Path,
    harness_errors: &mut Vec<String>,
) -> RecoveredState {
    let checkpoint = matches!(spec.scenario, RecoveryScenarioKind::Checkpoint);
    recover_twice_with(
        spec.engine,
        spec.durability,
        db_dir,
        checkpoint,
        harness_errors,
    )
}

pub(crate) fn recover_twice_with(
    engine: EngineKind,
    durability: DurabilityKind,
    db_dir: &Path,
    checkpoint_after_first: bool,
    harness_errors: &mut Vec<String>,
) -> RecoveredState {
    let first = match observe::observe(engine, durability, db_dir, checkpoint_after_first) {
        Ok(state) => state,
        Err(err) => {
            harness_errors.push(format!("first recovery pass: {err:#}"));
            return RecoveredState::default();
        }
    };
    match observe::observe(engine, durability, db_dir, false) {
        Ok(second) => harness_errors.extend(first.image_differences(&second)),
        Err(err) => harness_errors.push(format!("second recovery pass: {err:#}")),
    }
    first
}

/// Keep the tempdir of a failed run and describe it.
pub(crate) fn keep_evidence(
    tmp: tempfile::TempDir,
    ctx: &HarnessContext,
    stderr_name: &str,
) -> FailureEvidence {
    let dir = tmp.keep();
    FailureEvidence {
        stderr_tail: stderr_tail(&dir.join(stderr_name)),
        dir,
        seed: ctx.seed,
        git_sha: ctx.git_sha.clone(),
    }
}

pub(crate) fn stderr_tail(path: &Path) -> String {
    match fs::read_to_string(path) {
        Ok(text) => {
            let lines: Vec<&str> = text.lines().collect();
            lines[lines.len().saturating_sub(STDERR_TAIL_LINES)..].join("\n")
        }
        Err(err) => format!("<stderr unavailable: {err}>"),
    }
}

fn spawn_recovery_child(exe: &Path, args: &RecoverChildArgs, stderr: &Path) -> Result<Child> {
    let stderr = File::create(stderr)
        .with_context(|| format!("create child stderr file {}", stderr.display()))?;
    Command::new(exe)
        .arg("recover-child")
        .arg("--engine")
        .arg(engine_name(args.engine))
        .arg("--durability")
        .arg(args.durability.as_str())
        .arg("--scenario")
        .arg(args.scenario.as_str())
        .arg("--db-dir")
        .arg(&args.db_dir)
        .arg("--ack-log")
        .arg(&args.ack_log)
        .arg("--rows")
        .arg(args.rows.to_string())
        .arg("--checkpoint-every-rows")
        .arg(args.checkpoint_every_rows.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| {
            format!(
                "spawn recovery child {} for {:?}",
                exe.display(),
                args.engine
            )
        })
}

/// True once the child printed [`READY_LINE`]; false on EOF or timeout.
fn wait_ready(child: &mut Child, timeout: Duration) -> bool {
    let Some(stdout) = child.stdout.take() else {
        return false;
    };
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let mut signalled = false;
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) if !signalled && line.trim_end() == READY_LINE => {
                    let _ = tx.send(true);
                    signalled = true;
                }
                Ok(_) => {}
            }
        }
        if !signalled {
            let _ = tx.send(false);
        }
    });
    matches!(rx.recv_timeout(timeout), Ok(true))
}

struct ChildStop {
    status: String,
    kill_result: String,
    fault_observed: bool,
}

/// Kill the child if it is still running. The fault counts as observed
/// only when the kill landed on a live child and it died from SIGKILL; a
/// child that already exited (0 means it finished the workload) was never
/// crashed.
fn stop_child(child: &mut Child) -> ChildStop {
    match child.try_wait() {
        Ok(Some(status)) => ChildStop {
            status: format!("exited before the kill: {}", describe(status)),
            kill_result: "not sent".to_owned(),
            fault_observed: false,
        },
        Ok(None) => {
            let kill_result = match child.kill() {
                Ok(()) => "sent".to_owned(),
                Err(err) => format!("kill failed: {err}"),
            };
            match child.wait() {
                Ok(status) => ChildStop {
                    fault_observed: kill_result == "sent" && killed_by_parent(status),
                    status: describe(status),
                    kill_result,
                },
                Err(err) => ChildStop {
                    status: format!("wait failed: {err}"),
                    kill_result,
                    fault_observed: false,
                },
            }
        }
        Err(err) => ChildStop {
            status: format!("try_wait failed: {err}"),
            kill_result: "not sent".to_owned(),
            fault_observed: false,
        },
    }
}

#[cfg(unix)]
fn killed_by_parent(status: ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt;
    status.signal() == Some(libc::SIGKILL)
}

#[cfg(not(unix))]
fn killed_by_parent(status: ExitStatus) -> bool {
    !status.success()
}

pub(crate) fn describe(status: ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal({signal})");
        }
    }
    match status.code() {
        Some(code) => format!("exit({code})"),
        None => "unknown".to_owned(),
    }
}

pub fn default_matrix_checkpoint_every_rows() -> usize {
    32
}
