//! xtask — internal dev tools for redline-testing.
//!
//! Subcommands include:
//!
//!   * `generate` writes the matrix-product shards under
//!     `corpus/sqlite_parity/cases/gen_*.json`. Each shard is produced by a
//!     Rust generator: it enumerates a cartesian product of axes (functions,
//!     inputs, modifiers, etc.), runs the resulting SQL through the pinned
//!     reference `sqlite3 -batch -bail :memory:`, captures the actual stdout +
//!     exit code, and emits a JSON shard with that captured output as
//!     expected_stdout. By construction every generated case passes the
//!     reference self-compare.
//!
//!   * `generate --check` re-generates each shard in memory and asserts byte
//!     equality with the on-disk shard. Used in CI to keep rules and shards
//!     in sync.
//!
//!   * `ship-gate` walks the pinned `corpus/sqlite_parity/generated_manifest.json`
//!     and `corpus/sqlite_parity/cases/*.json`, runs each case through the
//!     pinned reference `sqlite3`, and verifies its declared
//!     expected_stdout, exit code, and substring contains-checks match.
//!     Failing case IDs are printed so they can be re-blessed or removed from
//!     the shard. This is the authoritative ship contract for the
//!     SQLite-parity corpus.
//!
//!   * Both default `--sqlite-bin` to the reference shell that
//!     `scripts/sqlite/build-reference.sh` builds (see `pinned_sqlite`) and
//!     refuse any shell that is not that stamped release.
//!
//!   * Repository CI helpers update the score badge and emit the cost,
//!     readiness, artifact-support, and canary-telemetry JSON contracts.

mod badge;
mod generators;
mod pinned_sqlite;
mod repo_ops;
mod sqlite_runner;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "xtask", about = "redline-testing dev tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Update the marked Jankurai badge block in README.md from the tracked
    /// agent/jankurai-badge.json source of truth.
    UpdateBadge,
    /// Validate the zero-spend policy and emit its machine-readable receipt.
    CostBudget,
    /// Validate launch-gate documentation and emit its readiness receipt.
    ReleaseReadiness,
    /// Validate that a Jain corrective tag matches Cargo.toml's product version.
    ValidateReleaseTag {
        #[arg(long)]
        tag: String,
    },
    /// Write the artifact-support context, manifest, and local-CI receipt.
    ArtifactSupportJson {
        #[arg(long)]
        out_dir: PathBuf,
        #[arg(long)]
        entrypoint: String,
        #[arg(long)]
        sha: String,
        #[arg(long)]
        tree: String,
        #[arg(long)]
        generated_at: String,
        #[arg(long)]
        workers: usize,
    },
    /// Read historical receipt files and emit their compatible telemetry JSON.
    Telemetry {
        #[arg(long)]
        repo: String,
        #[arg(long)]
        sha: String,
        #[arg(long)]
        ring_percent: u8,
        #[arg(long)]
        slug: String,
        #[arg(long)]
        store_root: PathBuf,
    },
    /// Regenerate the matrix-product SQLite-parity shards into
    /// corpus/sqlite_parity/cases/gen_*.json. With --check, refuses to
    /// write and instead fails if the on-disk shard differs.
    Generate {
        #[arg(long)]
        check: bool,
        /// Only regenerate the named rule (e.g. "math", "cast"). Default: all.
        #[arg(long)]
        only: Option<String>,
        /// SQLite shell used to capture expected outputs. Default: the pinned
        /// reference build; any shell must be the stamped pinned release.
        #[arg(long)]
        sqlite_bin: Option<PathBuf>,
    },
    /// Validate every case in every shard by running it through sqlite3 and
    /// asserting the declared expected behavior. Exits non-zero with the
    /// list of failing case IDs if anything diverges.
    ShipGate {
        /// Optional path to a single shard JSON; default: the pinned
        /// manifest and every shard under corpus/sqlite_parity/cases/.
        path: Option<PathBuf>,
        /// SQLite shell to validate against. Default: the pinned reference
        /// build; any shell must be the stamped pinned release.
        #[arg(long)]
        sqlite_bin: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let repo_root = find_repo_root()?;
    match cli.command {
        Command::UpdateBadge => badge::run(&repo_root),
        Command::CostBudget => repo_ops::cost_budget(&repo_root),
        Command::ReleaseReadiness => repo_ops::release_readiness(&repo_root),
        Command::ValidateReleaseTag { tag } => repo_ops::validate_release_tag(&repo_root, &tag),
        Command::ArtifactSupportJson {
            out_dir,
            entrypoint,
            sha,
            tree,
            generated_at,
            workers,
        } => repo_ops::artifact_support_json(
            &repo_root,
            &out_dir,
            &entrypoint,
            &sha,
            &tree,
            &generated_at,
            workers,
        ),
        Command::Telemetry {
            repo,
            sha,
            ring_percent,
            slug,
            store_root,
        } => repo_ops::telemetry(&repo, &sha, ring_percent, &slug, &store_root),
        Command::Generate {
            check,
            only,
            sqlite_bin,
        } => {
            let sqlite = oracle(&repo_root, sqlite_bin.as_deref())?;
            generators::run(&repo_root, &sqlite, only.as_deref(), check)
        }
        Command::ShipGate { path, sqlite_bin } => {
            let sqlite = oracle(&repo_root, sqlite_bin.as_deref())?;
            let corpus = repo_root.join("corpus").join("sqlite_parity");
            let targets = match path {
                Some(path) => vec![path],
                None => vec![corpus.join("generated_manifest.json"), corpus.join("cases")],
            };
            ship_gate(&targets, &sqlite)
        }
    }
}

/// The verified pinned reference shell, as the path string the case runner
/// takes.
fn oracle(repo_root: &Path, explicit: Option<&Path>) -> Result<String> {
    let pinned = pinned_sqlite::resolve(repo_root, explicit)?;
    eprintln!(
        "sqlite3 oracle: {} ({}), build stamp {:?}",
        pinned.path.display(),
        pinned.version,
        pinned.stamp.lines().next().unwrap_or_default()
    );
    pinned
        .path
        .to_str()
        .map(str::to_owned)
        .with_context(|| format!("non-UTF-8 sqlite3 path {}", pinned.path.display()))
}

fn ship_gate(targets: &[PathBuf], sqlite_bin: &str) -> Result<()> {
    let mut paths = Vec::new();
    for target in targets {
        paths.extend(shard_paths(target)?);
    }

    let mut total = 0usize;
    let mut failures: Vec<(String, u64, String, String)> = Vec::new();
    for shard in &paths {
        let body =
            fs::read_to_string(shard).with_context(|| format!("read shard {}", shard.display()))?;
        let cases: Vec<sqlite_runner::Case> = serde_json::from_str(&body)
            .with_context(|| format!("parse shard {}", shard.display()))?;
        for case in &cases {
            total += 1;
            if let Err(reason) = sqlite_runner::validate(case, sqlite_bin) {
                failures.push((
                    shard
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    case.id,
                    case.name.clone(),
                    reason.to_string(),
                ));
            }
        }
    }
    println!("ship-gate: {total} cases, {} failures", failures.len());
    for (shard, id, name, reason) in &failures {
        println!("  FAIL {shard} #{id} {name}: {reason}");
    }
    if failures.is_empty() {
        Ok(())
    } else {
        bail!("{} cases failed ship-gate", failures.len())
    }
}

/// A shard file itself, or every case shard in a directory (metadata shards,
/// named with a leading underscore, are not case lists).
fn shard_paths(target: &Path) -> Result<Vec<PathBuf>> {
    if target.is_file() {
        return Ok(vec![target.to_path_buf()]);
    }
    if !target.is_dir() {
        bail!("not a file or directory: {}", target.display());
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(target).with_context(|| format!("read {}", target.display()))? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "json")
            && !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('_'))
        {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn find_repo_root() -> Result<PathBuf> {
    // xtask is invoked from anywhere under the workspace; walk up looking
    // for the Cargo.toml that declares the `redline-testing` package or
    // workspace.
    let mut cwd = std::env::current_dir().context("cwd")?;
    loop {
        if cwd.join("corpus").join("sqlite_parity").is_dir() && cwd.join("xtask").is_dir() {
            return Ok(cwd);
        }
        if !cwd.pop() {
            bail!("could not locate redline-testing repo root from cwd");
        }
    }
}
