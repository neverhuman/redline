use std::{path::PathBuf, process};

use anyhow::Result;
use clap::{Parser, Subcommand};
use redlinedb_bench::perf_evidence::{
    self, RunPlan, SummaryOptions, W2ManifestInput, capture_w2_runtime_metadata,
};

#[derive(Debug, Parser)]
#[command(about = "Generate performance statistics and evidence in Rust")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Case-level latency statistics of a runner's raw JSONL. Every row
    /// must parse and occur once; with --expected-repetitions (publish
    /// mode) every executed case must have exactly that many measured
    /// repetitions, no duration may be unusable, and some case must be
    /// eligible.
    SummarizeJsonl {
        #[arg(long)]
        expected_repetitions: Option<usize>,
        /// Print the summary as JSON instead of text.
        #[arg(long)]
        json: bool,
        input: PathBuf,
    },
    /// Refuses a runner raw file that is not exactly the requested run:
    /// every case of the plan (and of --case-manifest, the runner's
    /// `list --suite sqlite_parity --format json` output) once, each
    /// executed case with exactly --warmup warmups and measured
    /// repetitions 1..=--reps, each sample once, and the runner's
    /// completion marker certifying the file.
    ValidateRun {
        #[arg(long)]
        expected_cases: usize,
        #[arg(long, alias = "repetitions")]
        reps: usize,
        #[arg(long)]
        warmup: usize,
        #[arg(long)]
        case_manifest: Option<PathBuf>,
        input: PathBuf,
    },
    AssertDistinctBinaries {
        target: PathBuf,
        reference: PathBuf,
    },
    AppendW2Manifest {
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        profile: String,
        #[arg(long)]
        allocator: String,
        #[arg(long)]
        label: String,
        #[arg(long)]
        binary: PathBuf,
        #[arg(long)]
        suite: String,
        #[arg(long)]
        perf_jsonl: Option<String>,
        /// The workload the PGO profile was trained on; omit for a build
        /// that used no profile.
        #[arg(long)]
        pgo_training_corpus: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        base_rustflags: String,
    },
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::SummarizeJsonl {
            expected_repetitions,
            json,
            input,
        } => {
            let summary = perf_evidence::summarize_jsonl_path(
                &input,
                SummaryOptions {
                    expected_repetitions,
                },
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&summary)?);
            } else {
                print!("{}", summary.render());
            }
        }
        Command::ValidateRun {
            expected_cases,
            reps,
            warmup,
            case_manifest,
            input,
        } => {
            let plan = RunPlan {
                expected_cases,
                repetitions: reps,
                warmup,
                case_manifest: case_manifest
                    .as_deref()
                    .map(perf_evidence::read_case_manifest)
                    .transpose()?,
            };
            print!(
                "{}",
                perf_evidence::validate_run_path(&input, &plan)?.render(&plan)
            );
        }
        Command::AssertDistinctBinaries { target, reference } => {
            perf_evidence::assert_distinct_binaries(&target, &reference)?;
        }
        Command::AppendW2Manifest {
            output,
            profile,
            allocator,
            label,
            binary,
            suite,
            perf_jsonl,
            pgo_training_corpus,
            base_rustflags,
        } => {
            let (captured_at_utc, rustc_version, host) = capture_w2_runtime_metadata()?;
            perf_evidence::append_w2_manifest(&W2ManifestInput {
                output_path: output,
                captured_at_utc,
                profile,
                allocator,
                label,
                binary_path: binary,
                suite,
                perf_jsonl,
                pgo_training_corpus,
                rustc_version,
                base_rustflags,
                host,
            })?;
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("perf evidence: {error:#}");
        process::exit(2);
    }
}
