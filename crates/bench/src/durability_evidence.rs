//! `durability-evidence`: a crash receipt for the shipped `redlinedb` shell
//! (workplan R10).
//!
//! The recover and failpoint lanes run a bench child that links the kernel
//! with failpoints. This command runs the binary a user installs instead. It
//! refuses a failpoint build, and it refuses tmpfs for Strict, where fsync
//! does nothing. Each scenario feeds the shell a long workload on stdin,
//! one two-row transaction per key, each followed by an ack line flushed to
//! the pipe after `COMMIT` returned. The harness writes each ack to its own
//! ledger and fsyncs it, sends SIGKILL at a seeded ack count plus a seeded
//! delay, drains the acks already in the pipe, and then opens the database
//! twice more with the same binary. The recover oracle
//! ([`crate::recover::oracle`]) grades the first recovery: every
//! acknowledged transaction present with its exact typed values, nothing
//! unacknowledged beyond the one in flight, no half transaction, the index
//! equal to a full scan, and `PRAGMA integrity_check` clean. The second
//! recovery must read back the same image.
//!
//! The receipt names the source commit and whether the tree was dirty, the
//! binary's SHA-256, the failpoint scan, the toolchain, the effective mode
//! as `PRAGMA redline_durability` reports it, the filesystem and its mount
//! options, the kernel, the seeds, the scenario counts, the SHA-256 of every
//! raw log, and the limits of what a SIGKILL proves. The command exits
//! non-zero unless every planned scenario passed.
//!
//! `durability-evidence-verify` checks receipts against the claim tags the
//! docs carry; `ops/ci/durability-claim-gate.sh` runs it at release.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::Args;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

#[path = "durability_evidence/preflight.rs"]
pub mod preflight;
#[path = "durability_evidence/readback.rs"]
pub mod readback;
#[path = "durability_evidence/receipt.rs"]
pub mod receipt;
#[path = "durability_evidence/session.rs"]
mod session;
#[path = "durability_evidence/verify.rs"]
mod verify;

use receipt::{EvidenceMode, FailureModel, KillWindow, Receipt, SCHEMA_VERSION, ScenarioCounts};

pub use verify::{DurabilityEvidenceVerifyArgs, read_receipt, verify};

pub const DEFAULT_SCENARIOS: usize = 10;

#[derive(Debug, Clone, Args)]
pub struct DurabilityEvidenceArgs {
    /// The shipped `redlinedb` shell. Build it on its own with `cargo build
    /// --release --locked -p redlinedb-cli`: building it together with
    /// redlinedb-bench turns kernel failpoints on, and the scan rejects that.
    #[arg(long)]
    pub binary: PathBuf,
    #[arg(long, value_enum)]
    pub mode: EvidenceMode,
    #[arg(long = "failure-model", value_enum)]
    pub failure_model: FailureModel,
    /// Where the receipt is written.
    #[arg(long)]
    pub out: PathBuf,
    /// Directory the databases and raw logs go under, on the filesystem
    /// the receipt is about. Each invocation makes a fresh `run-*`
    /// directory inside it.
    #[arg(long, default_value = "target/durability-evidence")]
    pub work_dir: PathBuf,
    /// Source tree the receipt names (commit and dirtiness).
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
    #[arg(long, default_value_t = DEFAULT_SCENARIOS)]
    pub scenarios: usize,
    #[arg(long, default_value_t = 7)]
    pub seed: u64,
    /// Fewest acks a scenario reads before its kill.
    #[arg(long, default_value_t = 8)]
    pub min_acks: usize,
    /// Most acks a scenario reads before its kill.
    #[arg(long, default_value_t = 256)]
    pub max_acks: usize,
    /// Longest extra delay after the chosen ack, in microseconds.
    #[arg(long, default_value_t = 3000)]
    pub max_delay_us: u64,
}

/// What SIGKILL on this harness does not show. Recorded in every receipt.
fn limitations(mode: EvidenceMode) -> Vec<String> {
    let mut out = vec![
        "failure model is SIGKILL of the shell process: the OS keeps every byte the process handed it, so this is not an OS crash or a power cut and says nothing about data in the page cache or a device cache".to_owned(),
        "one connection in one process runs sequential two-row transactions on one table pair with one index; no concurrent writers, no checkpoint or WAL segment rotation is forced during the workload".to_owned(),
        "the receipt records the binary's SHA-256 and the toolchain found on the host; it does not rebuild the binary from source_sha".to_owned(),
        "the result holds for the filesystem, mount options and kernel recorded here".to_owned(),
    ];
    if mode == EvidenceMode::Normal {
        out.push("Normal acknowledges after write(2), before fsync: surviving SIGKILL is expected, and nothing here speaks to power loss".to_owned());
    }
    out
}

fn plans(args: &DurabilityEvidenceArgs) -> Result<Vec<session::ScenarioPlan>> {
    if args.min_acks == 0 || args.min_acks > args.max_acks {
        bail!(
            "need 0 < --min-acks ({}) <= --max-acks ({})",
            args.min_acks,
            args.max_acks
        );
    }
    let mut rng = ChaCha8Rng::seed_from_u64(args.seed);
    // Enough rows that the child is still committing when the kill lands,
    // even if it runs thousands of commits ahead of the harness.
    let rows = args.max_acks + 4096;
    Ok((0..args.scenarios)
        .map(|index| session::ScenarioPlan {
            index,
            seed: rng.random(),
            kill_after_acks: rng.random_range(args.min_acks..=args.max_acks),
            kill_delay_us: rng.random_range(0..=args.max_delay_us),
            rows,
        })
        .collect())
}

pub fn run(args: &DurabilityEvidenceArgs) -> Result<Receipt> {
    let plans = plans(args)?;
    let repo = args
        .repo
        .canonicalize()
        .with_context(|| format!("source tree {}", args.repo.display()))?;
    // Before anything is written, so the receipt's own output cannot make
    // the tree look dirty.
    let (source_sha, dirty) = preflight::source(&repo);
    let scan = preflight::scan_binary(&args.binary)?;
    let toolchain = preflight::toolchain(&repo);
    let created_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let run_dir = args
        .work_dir
        .join(format!("run-{created_unix}-{}", args.seed));
    fs::create_dir_all(&run_dir).with_context(|| format!("create {}", run_dir.display()))?;
    let run_dir = run_dir.canonicalize()?;

    let mut rejections = Vec::new();
    if !scan.failpoints.clean {
        rejections.push(format!(
            "{} contains kernel failpoint strings {:?}: it is a failpoint build, not the shipped binary; build it alone with `cargo build --release --locked -p redlinedb-cli`",
            args.binary.display(),
            scan.failpoints.markers_found
        ));
    }
    let filesystem = match preflight::filesystem(&run_dir) {
        Ok(fs) => Some(fs),
        Err(err) => {
            rejections.push(format!("cannot name the filesystem: {err:#}"));
            None
        }
    };
    if let Some(fs) = &filesystem
        && args.mode == EvidenceMode::Strict
        && fs.is_memory_backed()
    {
        rejections.push(format!(
            "{} is on {} ({}), where fsync does nothing; a Strict receipt must be taken on the disk it is about",
            fs.work_dir, fs.fs_type, fs.mount_point
        ));
    }
    if let (Some(built), Some(host)) = (&scan.facts.rustc_commit, &toolchain.rustc_commit)
        && built != host
    {
        rejections.push(format!(
            "the binary was built by rustc {built}, but this tree's rustc is {host}"
        ));
    }
    if args.scenarios == 0 {
        rejections.push("--scenarios 0 plans nothing".to_owned());
    }

    let mut runs = Vec::new();
    if rejections.is_empty() {
        for plan in &plans {
            let dir = run_dir.join(format!("scenario-{:03}", plan.index));
            let run = session::run_scenario(&args.binary, args.mode, plan, &dir)?;
            eprintln!(
                "durability-evidence: {} scenario {} kill after {} acks +{}us: acked {} ({} drained), {}",
                if run.passed { "PASS" } else { "FAIL" },
                plan.index,
                plan.kill_after_acks,
                plan.kill_delay_us,
                run.acknowledged,
                run.acks_drained_after_kill,
                run.verdict.summary()
            );
            runs.push(run);
        }
    }

    let passed_runs = runs.iter().filter(|run| run.passed).count();
    let modes: Vec<&String> = runs.iter().flat_map(|run| &run.effective_modes).collect();
    let effective_mode = match modes.first() {
        Some(first) if modes.iter().all(|mode| mode == first) && !first.is_empty() => {
            Some((*first).clone())
        }
        _ => None,
    };
    let scenarios = ScenarioCounts {
        planned: plans.len(),
        run: runs.len(),
        passed: passed_runs,
        failed: runs.len() - passed_runs,
        faults_observed: runs.iter().filter(|run| run.fault_observed).count(),
    };
    let passed = rejections.is_empty()
        && scenarios.planned > 0
        && scenarios.run == scenarios.planned
        && scenarios.passed == scenarios.planned;
    Ok(Receipt {
        schema_version: SCHEMA_VERSION,
        tool: format!("redlinedb-bench {} durability-evidence", env!("CARGO_PKG_VERSION")),
        created_unix,
        source_sha,
        dirty,
        binary: scan.facts,
        failpoint_scan: scan.failpoints,
        toolchain,
        host: preflight::host(),
        mode: args.mode,
        effective_mode,
        failure_model: args.failure_model,
        filesystem,
        child_environment: session::child_environment(args.mode)
            .into_iter()
            .map(|(name, _)| name)
            .collect(),
        oracle: "recover::oracle (workload `wal`): exact typed digest of every acknowledged transaction, at most the one in-flight key beyond them, no partial transaction, kv_tenant_idx equal to a NOT INDEXED scan, PRAGMA integrity_check ok, second recovery identical to the first".to_owned(),
        seed: args.seed,
        kill_window: KillWindow {
            min_acks: args.min_acks,
            max_acks: args.max_acks,
            max_delay_us: args.max_delay_us,
        },
        scenarios,
        rejections,
        runs,
        passed,
        limitations: limitations(args.mode),
        logs_dir: Some(run_dir.display().to_string()),
    })
}

/// Write the receipt, then fail unless it passed.
pub fn write_and_check(out: &Path, receipt: &Receipt) -> Result<()> {
    crate::report::write_json(Some(out), receipt)?;
    if receipt.passed {
        eprintln!(
            "durability-evidence: {}/{} scenarios passed; receipt {}",
            receipt.scenarios.passed,
            receipt.scenarios.planned,
            out.display()
        );
        return Ok(());
    }
    let mut reasons = receipt.rejections.clone();
    for run in receipt.runs.iter().filter(|run| !run.passed) {
        reasons.push(format!("scenario {}: {}", run.index, run.verdict.summary()));
    }
    bail!(
        "durability-evidence failed (passed=false, {}/{} scenarios passed); receipt {}:\n{}",
        receipt.scenarios.passed,
        receipt.scenarios.planned,
        out.display(),
        reasons.join("\n")
    )
}
