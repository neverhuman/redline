//! The command line.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::image::ImageKind;
use crate::pair::{Engine, Pair};
use crate::{case, render, run, summary, workloads};

#[derive(Parser)]
#[command(name = "redline-scoreboard", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the workload catalog as JSON.
    List,
    /// Print the engine versions this binary links, as JSON.
    Info,
    /// Summarize a bundle into its summary.json, or check that it is current.
    Summarize {
        #[arg(long)]
        bundle: PathBuf,
        #[arg(long)]
        check: bool,
    },
    /// Render a bundle into a file's engine-throughput block, or check it.
    Render {
        /// The bundle, as a path relative to the repository root.
        #[arg(long)]
        bundle: String,
        #[arg(long)]
        target: Vec<PathBuf>,
        #[arg(long)]
        check: bool,
    },
    /// Pair Strict versions per repetition, with their SQLite controls adjacent.
    StrictPair(crate::strict::Args),
    /// Measure every selected workload and append one record per repetition.
    Run {
        /// Label for the RedlineDB records (the version under test).
        #[arg(long)]
        label: String,
        /// Run number within a bundle.
        #[arg(long, default_value_t = 1)]
        run: u32,
        #[arg(long, default_value_t = 20_000)]
        rows: u64,
        #[arg(long, value_enum, default_value_t = Pair::Normal)]
        pair: Pair,
        #[arg(long, default_value_t = 5)]
        reps: u32,
        /// Divide each workload's fixed work by this. Only 1 is publishable.
        #[arg(long, default_value_t = 1)]
        work_divisor: u64,
        #[arg(long, value_enum, value_delimiter = ',', default_values_t = [Engine::Redline, Engine::Sqlite])]
        engines: Vec<Engine>,
        /// Comma-separated workload ids; all of them when omitted.
        #[arg(long, value_delimiter = ',')]
        workloads: Option<Vec<String>>,
        /// Where images are built once and kept.
        #[arg(long)]
        image_store: PathBuf,
        /// Where each case copies its image.
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// Seconds a case may take before it is recorded as timed out.
        #[arg(long, default_value_t = 600)]
        budget_s: u64,
        /// Seconds an image build may take.
        #[arg(long, default_value_t = 3_600)]
        image_budget_s: u64,
    },
    /// Build one image (a child of `run`).
    Image {
        #[arg(long, value_enum)]
        engine: Engine,
        #[arg(long, value_enum)]
        kind: ImageKind,
        #[arg(long)]
        rows: u64,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        base: Option<PathBuf>,
    },
    /// Measure one repetition of one workload (a child of `run`).
    Case {
        #[arg(long, value_enum)]
        engine: Engine,
        #[arg(long)]
        workload: String,
        #[arg(long)]
        rows: u64,
        #[arg(long, value_enum)]
        pair: Pair,
        #[arg(long, default_value_t = 1)]
        work_divisor: u64,
        #[arg(long)]
        image: PathBuf,
        #[arg(long)]
        work: PathBuf,
    },
}

/// Parse the command line and run the command.
pub fn main() -> Result<()> {
    match Cli::parse().command {
        Command::List => {
            println!("{}", serde_json::to_string_pretty(workloads::CATALOG)?);
        }
        Command::Info => {
            let source_id: String = rusqlite::Connection::open_in_memory()?.query_row(
                "SELECT sqlite_source_id()",
                [],
                |row| row.get(0),
            )?;
            println!(
                "{}",
                serde_json::json!({
                    "redlinedb": env!("CARGO_PKG_VERSION"),
                    "sqlite": rusqlite::version(),
                    "sqlite_source_id": source_id,
                })
            );
        }
        Command::Summarize { bundle, check } => {
            let summary = summary::write_or_check(&bundle, check)?;
            if summary.publishable {
                println!("{}: publishable", summary.bundle);
            } else {
                println!("{}: not publishable", summary.bundle);
                for blocker in &summary.blockers {
                    println!("  blocker: {blocker}");
                }
            }
        }
        Command::Render {
            bundle,
            target,
            check,
        } => {
            let summary = summary::write_or_check(std::path::Path::new(&bundle), true)?;
            for file in &target {
                render::render_into(&summary, &bundle, file, check)?;
            }
        }
        Command::StrictPair(args) => crate::strict::run(&args)?,
        Command::Run {
            label,
            run,
            rows,
            pair,
            reps,
            work_divisor,
            engines,
            workloads,
            image_store,
            work_root,
            out,
            budget_s,
            image_budget_s,
        } => run::run(&run::RunArgs {
            label,
            run,
            rows,
            pair,
            reps,
            divisor: work_divisor,
            engines,
            workloads,
            image_store,
            work_root,
            out,
            budget: Duration::from_secs(budget_s),
            image_budget: Duration::from_secs(image_budget_s),
        })?,
        Command::Image {
            engine,
            kind,
            rows,
            out,
            base,
        } => case::build_image(engine, kind, rows, &out, base.as_deref())?,
        Command::Case {
            engine,
            workload,
            rows,
            pair,
            work_divisor,
            image,
            work,
        } => {
            let workload = workloads::find(&workload)?;
            let measured =
                case::run_case(engine, &workload, rows, pair, work_divisor, &image, &work)?;
            println!("{}", serde_json::to_string(&measured)?);
        }
    }
    Ok(())
}
