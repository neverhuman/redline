use std::{path::PathBuf, process};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use redlinedb_bench::perf_evidence::{
    self, BuildContractInput, DeclaredBuild, RunPlan, SummaryOptions, W2ManifestInput,
    capture_w2_runtime_metadata,
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
    /// Writes build-contract.json for a measured target: rustc -vV, the
    /// target, reference and runner digests and versions, the reference's
    /// PRAGMA compile_options, and the target's build as its builder
    /// declared it. Without --profile, --features or --rustflags the build
    /// is recorded as undeclared.
    BuildContract {
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        target_bin: PathBuf,
        #[arg(long)]
        reference_bin: PathBuf,
        #[arg(long)]
        runner_bin: PathBuf,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        features: Option<String>,
        /// The RUSTFLAGS the target was built with; `--rustflags=` for a
        /// build with none.
        #[arg(long, allow_hyphen_values = true)]
        rustflags: Option<String>,
        #[arg(long)]
        pgo_training_corpus: Option<String>,
    },
    /// Writes a release bench bundle's summary.json (L-05) from the
    /// bundle's own files: every run complete and accepted, one reference
    /// and one runner, and per label the corpus pass count, the common pass
    /// set and the latency-ratio median and p95 with their spread across
    /// runs. With --check, fails unless summary.json already holds exactly
    /// that.
    SummarizeBundle {
        #[arg(long)]
        bundle: PathBuf,
        #[arg(long)]
        check: bool,
    },
    /// Prints a case list's ids (one numeric id per line, `#` comments
    /// allowed) as five-digit display ids, one per line.
    CaseListIds {
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
        /// The RUSTFLAGS the variant was built with; omit when unknown.
        #[arg(long, allow_hyphen_values = true)]
        rustflags: Option<String>,
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
        Command::BuildContract {
            output,
            target_bin,
            reference_bin,
            runner_bin,
            profile,
            features,
            rustflags,
            pgo_training_corpus,
        } => {
            let declared = profile.is_some() || features.is_some() || rustflags.is_some();
            perf_evidence::write_build_contract(&BuildContractInput {
                output: output.clone(),
                target_bin,
                reference_bin,
                runner_bin,
                build: DeclaredBuild {
                    declared,
                    profile,
                    features,
                    rustflags,
                },
                pgo_training_corpus,
            })?;
            println!("wrote {}", output.display());
        }
        Command::SummarizeBundle { bundle, check } => {
            let summary = perf_evidence::write_bundle_summary(&bundle, check)?;
            println!(
                "{} {}: {} labels x {} runs, common pass set {} of {} cases, publishable {}",
                if check { "checked" } else { "wrote" },
                bundle.join("summary.json").display(),
                summary.labels.len(),
                summary.runs_per_label,
                summary.common_pass_set.cases,
                summary.corpus.cases,
                summary.publishable
            );
            for blocker in &summary.publication_blockers {
                println!("  not publishable: {blocker}");
            }
        }
        Command::CaseListIds { input } => {
            let text = std::fs::read_to_string(&input)
                .with_context(|| format!("read case list {}", input.display()))?;
            for id in perf_evidence::parse_case_list(&text)
                .with_context(|| format!("case list {}", input.display()))?
            {
                println!("{id}");
            }
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
            rustflags,
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
                rustflags,
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
