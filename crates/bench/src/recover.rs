//! Crash-recovery qualification: `recover` and `recover-matrix` kill a
//! child mid-workload and grade the recovered database with [`oracle`].
//! Both commands fail (non-zero exit) unless every run qualifies.

use std::fs;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::{
    DurabilityKind, EngineKind, RecoverArgs, RecoverChildArgs, RecoverMatrixArgs,
    RecoveryMatrixConfig, RecoveryScenarioKind,
};

#[path = "recover/child.rs"]
mod child;

#[path = "recover/harness.rs"]
pub mod harness;
#[path = "recover/observe.rs"]
pub(crate) mod observe;
#[path = "recover/oracle.rs"]
pub mod oracle;
#[path = "recover/units.rs"]
mod units;

use harness::{CaseOutcome, CaseSpec, HarnessContext};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryReport {
    pub seed: u64,
    pub git_sha: Option<String>,
    /// True only when there is at least one run and every run qualified.
    pub passed: bool,
    pub runs: Vec<RecoveryRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryRun {
    pub engine: EngineKind,
    pub durability: DurabilityKind,
    pub scenario: RecoveryScenarioKind,
    pub passed: bool,
    #[serde(flatten)]
    pub outcome: CaseOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryMatrixReport {
    pub seed: u64,
    pub git_sha: Option<String>,
    /// True only when there is at least one run and every run qualified.
    pub passed: bool,
    pub failed_cases: usize,
    pub runs: Vec<RecoveryMatrixRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryMatrixRun {
    pub case: String,
    pub engine: EngineKind,
    pub durability: DurabilityKind,
    pub scenario: RecoveryScenarioKind,
    pub kill_after_ms: u64,
    pub passed: bool,
    #[serde(flatten)]
    pub outcome: CaseOutcome,
}

impl RecoveryReport {
    pub fn from_runs(seed: u64, git_sha: Option<String>, runs: Vec<RecoveryRun>) -> Self {
        let passed = !runs.is_empty() && runs.iter().all(|run| run.passed);
        Self {
            seed,
            git_sha,
            passed,
            runs,
        }
    }

    /// Err unless the report has runs and every one of them qualified.
    pub fn ensure_passed(&self) -> Result<()> {
        ensure_all_passed(
            "recover",
            self.passed,
            self.runs.iter().map(|run| {
                (
                    format!("{:?}/{}", run.engine, run.durability.as_str()),
                    run.passed,
                    run.outcome.verdict.summary(),
                )
            }),
        )
    }
}

impl RecoveryMatrixReport {
    pub fn from_runs(seed: u64, git_sha: Option<String>, runs: Vec<RecoveryMatrixRun>) -> Self {
        let failed_cases = runs.iter().filter(|run| !run.passed).count();
        let passed = !runs.is_empty() && failed_cases == 0;
        Self {
            seed,
            git_sha,
            passed,
            failed_cases,
            runs,
        }
    }

    /// Err unless the report has runs and every one of them qualified.
    pub fn ensure_passed(&self) -> Result<()> {
        ensure_all_passed(
            "recover-matrix",
            self.passed,
            self.runs.iter().map(|run| {
                (
                    format!(
                        "{}/{:?}/{}/{}ms",
                        run.case,
                        run.engine,
                        run.durability.as_str(),
                        run.kill_after_ms
                    ),
                    run.passed,
                    run.outcome.verdict.summary(),
                )
            }),
        )
    }
}

fn ensure_all_passed(
    gate: &str,
    reported: bool,
    runs: impl Iterator<Item = (String, bool, String)>,
) -> Result<()> {
    let runs: Vec<(String, bool, String)> = runs.collect();
    if runs.is_empty() {
        bail!("{gate} gate failed: the report has no runs");
    }
    let failed: Vec<String> = runs
        .iter()
        .filter(|(_, passed, _)| !passed)
        .map(|(label, _, why)| format!("{label}: {why}"))
        .collect();
    if !failed.is_empty() {
        bail!(
            "{gate} gate failed: {}/{} runs did not qualify:\n{}",
            failed.len(),
            runs.len(),
            failed.join("\n")
        );
    }
    if !reported {
        bail!("{gate} gate failed: the report says passed=false");
    }
    Ok(())
}

pub fn run(args: &RecoverArgs) -> Result<RecoveryReport> {
    let ctx = HarnessContext::new(args.child_exe.as_deref(), args.seed)?;
    let mut runs = Vec::new();
    for &engine in args.engine.expand() {
        let spec = CaseSpec {
            label: "single",
            engine,
            durability: args.durability,
            scenario: RecoveryScenarioKind::Wal,
            rows: 1 << 20,
            checkpoint_every_rows: harness::default_matrix_checkpoint_every_rows(),
            kill_after: Duration::from_secs(args.seconds.max(1)),
        };
        let outcome = harness::run_case(&ctx, &spec)?;
        runs.push(RecoveryRun {
            engine,
            durability: args.durability,
            scenario: spec.scenario,
            passed: outcome.verdict.qualified,
            outcome,
        });
    }
    Ok(RecoveryReport::from_runs(ctx.seed, ctx.git_sha, runs))
}

pub fn run_matrix(args: &RecoverMatrixArgs) -> Result<RecoveryMatrixReport> {
    let raw = fs::read_to_string(&args.config)
        .with_context(|| format!("read recovery matrix {}", args.config.display()))?;
    let matrix = toml::from_str::<RecoveryMatrixConfig>(&raw)
        .with_context(|| format!("parse recovery matrix {}", args.config.display()))?;
    if matrix.cases.is_empty() {
        bail!("recovery matrix must define at least one case");
    }
    let ctx = HarnessContext::new(args.child_exe.as_deref(), args.seed)?;

    let mut runs = Vec::new();
    for &engine in args.engine.expand() {
        for &durability in &matrix.durabilities {
            for case in &matrix.cases {
                for &kill_after_ms in &case.kill_windows_ms {
                    let spec = CaseSpec {
                        label: &case.name,
                        engine,
                        durability,
                        scenario: case.scenario,
                        rows: case.rows,
                        checkpoint_every_rows: case.checkpoint_every_rows,
                        kill_after: Duration::from_millis(kill_after_ms.max(1)),
                    };
                    let outcome = harness::run_case(&ctx, &spec)?;
                    eprintln!(
                        "recover-matrix: {} {}/{:?}/{}/{kill_after_ms}ms acked={} {}",
                        if outcome.verdict.qualified {
                            "PASS"
                        } else {
                            "FAIL"
                        },
                        case.name,
                        engine,
                        durability.as_str(),
                        outcome.acknowledged,
                        outcome.verdict.summary()
                    );
                    runs.push(RecoveryMatrixRun {
                        case: case.name.clone(),
                        engine,
                        durability,
                        scenario: case.scenario,
                        kill_after_ms,
                        passed: outcome.verdict.qualified,
                        outcome,
                    });
                }
            }
        }
    }
    Ok(RecoveryMatrixReport::from_runs(ctx.seed, ctx.git_sha, runs))
}

/// The child reports READY only after its first commit and acknowledgement.
/// This makes the kill window cover a recovery workload even on a busy host.
pub fn run_child(args: &RecoverChildArgs) -> Result<()> {
    child::run(args)
}
