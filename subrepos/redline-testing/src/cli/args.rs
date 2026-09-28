use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "redline-testing")]
#[command(about = "Official RedlineDB conformance and benchmark runner")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub(crate) command: CommandKind,
}

#[derive(Debug, Subcommand)]
pub(crate) enum CommandKind {
    Run(RunArgs),
    CheckPostgres(CheckPostgresArgs),
    CheckSqlite(CheckSqliteArgs),
    Report(ReportArgs),
    List(ListArgs),
    JankuraiCompare(JankuraiCompareArgs),
    Sentinel(SentinelArgs),
    VersionHistory(VersionHistoryArgs),
    Version,
}

/// Renders the README "Versions over time" table from a release bench
/// bundle's summary.json (RedlineDB's scripts/perf/release-bench.sh and
/// perf_evidence summarize-bundle). Without --readme the block is printed;
/// with it, the block between `<!-- version-history:begin -->` and
/// `<!-- version-history:end -->` is replaced, and only from a publishable
/// bundle.
#[derive(Debug, Args)]
pub(crate) struct VersionHistoryArgs {
    #[arg(long)]
    pub(crate) bundle: PathBuf,
    #[arg(long)]
    pub(crate) readme: Option<PathBuf>,
    /// Fail unless the README block already holds exactly the rendered table.
    #[arg(long, requires = "readme")]
    pub(crate) check: bool,
}

#[derive(Debug, Args)]
pub(crate) struct RunArgs {
    #[arg(long, value_enum, default_value = "all")]
    pub(crate) suite: Suite,
    #[arg(long)]
    pub(crate) target_bin: PathBuf,
    #[arg(long, default_value = "auto")]
    pub(crate) sqlite_bin: String,
    #[arg(long, default_value = "auto")]
    pub(crate) workers: String,
    #[arg(long, default_value = "auto")]
    pub(crate) tmp_root: String,
    #[arg(long)]
    pub(crate) output: PathBuf,
    #[arg(long, default_value_t = 1)]
    pub(crate) repetitions: usize,
    #[arg(long, default_value_t = 0)]
    pub(crate) warmup: usize,
    /// Which engine runs first in each sample: `sqlite-first` (the
    /// correctness lane's order), `target-first`, or `alternate` (the
    /// reference first on even sample indexes, warmups included, and the
    /// target first on odd ones). Every raw record and suite manifest
    /// records it.
    #[arg(long, value_enum, default_value = "sqlite-first")]
    pub(crate) order: crate::sqlite_parity::MeasurementOrder,
    #[arg(long, value_enum, default_value = "auto")]
    pub(crate) progress: ProgressMode,
    #[arg(long)]
    pub(crate) memory_samples: bool,
    /// Explicit regression policy; failures remain failed in qualification output.
    #[arg(long)]
    pub(crate) postgres_regression_baseline: Option<PathBuf>,
    /// Update the marked README block from validated PostgreSQL results.
    /// Refused unless the evidence is release evidence (a clean source tree
    /// at a recorded commit and a measured reference identity).
    #[arg(long)]
    pub(crate) postgres_readme: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) publication: PublicationArgs,
    /// The sqlite_parity and memory cases known to fail. Listed failures are
    /// published as failures; an unlisted failure, or a listed case that now
    /// passes, fails the run after its evidence is written. Without it, any
    /// failure fails the run.
    #[arg(long)]
    pub(crate) sqlite_known_failures: Option<PathBuf>,
    /// Kill an engine run's whole process group once it has run this long;
    /// the sample is recorded as an execution failure (`timeout`).
    #[arg(long, default_value_t = 60_000)]
    pub(crate) case_timeout_ms: u64,
    /// Kill an engine run's whole process group once it has written more
    /// than this many bytes to stdout or to stderr (`output_limit`).
    #[arg(long, default_value_t = 16 * 1024 * 1024)]
    pub(crate) max_output_bytes: usize,
    /// Run only these case ids (repeatable). A diagnostic narrowing of one
    /// SQLite-shell suite; never official evidence.
    #[arg(long = "case-id", value_name = "ID")]
    pub(crate) case_ids: Vec<String>,
    /// An official run (the RedlineDB official lane): `--suite all` over the
    /// whole corpus with a known-failures baseline, no case narrowing, no
    /// `REDLINE_TESTING_PINNED_ONLY`, and no expired scope-policy exception.
    /// Only its evidence is publishable.
    #[arg(long)]
    pub(crate) official: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SelectArgs {
    #[arg(long)]
    pub(crate) priorities: Option<String>,
    #[arg(long)]
    pub(crate) profiles: Option<String>,
    #[arg(long)]
    pub(crate) include_quarantine: bool,
    #[arg(long)]
    pub(crate) case_list: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct ReportArgs {
    /// The one suite to render; `all` is refused (see `report::generate`).
    #[arg(long, value_enum)]
    pub(crate) suite: Suite,
    #[command(flatten)]
    pub(crate) select: SelectArgs,
    #[arg(long)]
    pub(crate) input: PathBuf,
    #[arg(long)]
    pub(crate) official_evidence: Option<PathBuf>,
    /// The run's own provenance (target/redline-testing/provenance.json,
    /// staged as run-provenance.json), bound by its hash in the evidence.
    #[arg(long, conflicts_with = "historical_run")]
    pub(crate) run_provenance: Option<PathBuf>,
    /// Render official evidence from a run that recorded no run provenance;
    /// the report is marked historical. Refused for evidence that names one.
    #[arg(long)]
    pub(crate) historical_run: bool,
    #[arg(long)]
    pub(crate) local_diagnostics: bool,
    #[arg(long)]
    pub(crate) out_dir: PathBuf,
    #[arg(long)]
    pub(crate) readme: PathBuf,
    #[arg(long)]
    pub(crate) plot: Option<PathBuf>,
    #[arg(long)]
    pub(crate) performance_histogram_plot: Option<PathBuf>,
    #[arg(long)]
    pub(crate) median_test_performance_plot: Option<PathBuf>,
    /// Keep the README block to correctness: no latency summary, plots or
    /// ranked latency table (the official lane's latencies measure a shared
    /// host running every case at once).
    #[arg(long)]
    pub(crate) no_readme_latency: bool,
    #[arg(long)]
    pub(crate) jankurai_score: Option<PathBuf>,
    #[arg(long)]
    pub(crate) updated_date: String,
    #[arg(long)]
    pub(crate) expected_repetitions: Option<usize>,
    #[arg(long)]
    pub(crate) expected_warmup: Option<usize>,
    #[arg(long)]
    pub(crate) check: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ListArgs {
    #[arg(long, value_enum, default_value = "all")]
    pub(crate) suite: Suite,
    #[command(flatten)]
    pub(crate) select: SelectArgs,
    #[arg(long, value_enum, default_value = "text")]
    pub(crate) format: ListFormat,
}

#[derive(Debug, Args)]
pub(crate) struct JankuraiCompareArgs {
    #[arg(long)]
    pub(crate) redlinedb_score: PathBuf,
    #[arg(long)]
    pub(crate) sqlite_score: PathBuf,
    #[arg(long)]
    pub(crate) sqlite_ref: String,
    #[arg(long)]
    pub(crate) updated_date: String,
    #[arg(long)]
    pub(crate) json: PathBuf,
    #[arg(long)]
    pub(crate) csv: PathBuf,
    #[arg(long)]
    pub(crate) check: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SentinelArgs {
    #[arg(long)]
    pub(crate) input: PathBuf,
    #[arg(long = "ceiling-ns")]
    pub(crate) ceiling_ns: Vec<String>,
    #[arg(long)]
    pub(crate) enforce: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum Suite {
    All,
    #[value(name = "sqlite_parity", alias = "sqlite-parity")]
    SqliteParity,
    #[value(name = "memory")]
    Memory,
    #[value(name = "rql_phase1", alias = "rql-phase1")]
    RqlPhase1,
    #[value(name = "beyond_sqlite", alias = "beyond-sqlite")]
    BeyondSqlite,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum ProgressMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum ListFormat {
    Text,
    Markdown,
    Json,
}

impl Suite {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::SqliteParity => "sqlite_parity",
            Self::Memory => "memory",
            Self::RqlPhase1 => "rql_phase1",
            Self::BeyondSqlite => "beyond_sqlite",
        }
    }
}

/// Reduces an official run's SQLite-shell suites to one verdict per case
/// and writes sqlite-qualification.json (SQ-04).
#[derive(Debug, Args)]
pub(crate) struct CheckSqliteArgs {
    /// The run's official-evidence.json; raw paths are relative to it.
    #[arg(long)]
    pub(crate) official_evidence: PathBuf,
    #[arg(long)]
    pub(crate) output: PathBuf,
}

#[derive(Debug, Args)]
pub(crate) struct CheckPostgresArgs {
    #[arg(long)]
    pub(crate) input: PathBuf,
    #[arg(long)]
    pub(crate) baseline: Option<PathBuf>,
    /// Update the marked README block. Refused unless the evidence is
    /// release evidence (a clean source tree at a recorded commit and a
    /// measured reference identity).
    #[arg(long)]
    pub(crate) readme: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) publication: PublicationArgs,
}

/// What the PostgreSQL gate requires before its result may be published.
#[derive(Debug, Args)]
pub(crate) struct PublicationArgs {
    /// The commit the run must have measured; the gate fails when the
    /// provenance records any other.
    #[arg(long, value_name = "SHA")]
    pub(crate) expected_source_commit: Option<String>,
    /// Fail the PostgreSQL gate unless its evidence is release evidence,
    /// even when no README is written.
    #[arg(long)]
    pub(crate) require_clean: bool,
}

impl PublicationArgs {
    pub(crate) fn policy(&self) -> crate::beyond_sqlite::gate::Publication {
        crate::beyond_sqlite::gate::Publication {
            expected_source_commit: self.expected_source_commit.clone(),
            require_clean: self.require_clean,
        }
    }
}
