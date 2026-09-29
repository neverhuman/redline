//! One scenario: run the shipped shell on the workload, SIGKILL it at the
//! planned point, then recover twice with the same binary and grade the
//! result with the recover oracle.

#[path = "session/child.rs"]
mod child;
#[path = "session/workload.rs"]
mod workload;

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::recover::harness::describe;
use crate::recover::oracle::{self, AckLedger};

use super::preflight::sha256_hex;
use super::readback::{self, WORKLOAD};
use super::receipt::{EvidenceMode, LogDigest, ScenarioRun};

use workload::drive_workload;

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
