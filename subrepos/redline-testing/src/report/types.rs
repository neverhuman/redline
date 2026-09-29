use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub struct ReportOptions {
    pub suite: String,
    pub input: PathBuf,
    pub official_evidence: Option<PathBuf>,
    /// The run provenance bound to `official_evidence` (official mode).
    pub run_provenance: Option<PathBuf>,
    /// `official_evidence` comes from a run that recorded no run provenance.
    pub historical_run: bool,
    pub local_diagnostics: bool,
    pub out_dir: PathBuf,
    pub readme: PathBuf,
    pub plot: Option<PathBuf>,
    pub performance_histogram_plot: Option<PathBuf>,
    pub median_test_performance_plot: Option<PathBuf>,
    /// Put the latency summary, latency plots and the ranked latency table
    /// in the README block. The official lane runs every case at once on a
    /// shared host, so its latencies measure contention; the README then
    /// shows correctness only and points at the release benchmark.
    pub readme_latency: bool,
    pub jankurai_score: Option<PathBuf>,
    pub updated_date: String,
    pub expected_repetitions: Option<usize>,
    pub expected_warmup: Option<usize>,
    pub check: bool,
    /// The case ids the raw records must cover exactly (SQ-04). `None` is
    /// the suite's compiled-in corpus for official evidence, and the cases
    /// the records hold for a local diagnostic.
    pub case_manifest: Option<std::collections::BTreeSet<String>>,
}

#[derive(Debug)]
pub struct JankuraiCompareOptions {
    pub redlinedb_score: PathBuf,
    pub sqlite_score: PathBuf,
    pub sqlite_ref: String,
    pub updated_date: String,
    pub json: PathBuf,
    pub csv: PathBuf,
    pub check: bool,
}

#[derive(Debug)]
pub struct SentinelOptions {
    pub input: PathBuf,
    pub ceiling_ns: Vec<String>,
    pub enforce: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawRecord {
    pub(crate) case_id: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) case_file: String,
    pub(crate) priority: String,
    pub(crate) profile: String,
    pub(crate) category: String,
    #[serde(default)]
    pub(crate) sample_role: String,
    #[serde(default)]
    pub(crate) repetition_index: Option<usize>,
    /// The sample's position in its case (warmups first); older records
    /// may lack it.
    #[serde(default)]
    pub(crate) sample_index: Option<usize>,
    pub(crate) status: String,
    pub(crate) reference_elapsed_ns: u128,
    pub(crate) target_elapsed_ns: u128,
    /// Executable identities the runner records on every executed sample;
    /// a declared skip carries empty strings.
    #[serde(default)]
    pub(crate) reference_executable_sha256: String,
    #[serde(default)]
    pub(crate) target_executable_sha256: String,
    #[serde(default)]
    pub(crate) reference_version: String,
    #[serde(default)]
    pub(crate) memory_status: String,
    #[serde(default)]
    pub(crate) reference_peak_rss_kb: Option<u64>,
    #[serde(default)]
    pub(crate) reference_rss_sampled_kb: Option<u64>,
    #[serde(default)]
    pub(crate) target_peak_rss_kb: Option<u64>,
    #[serde(default)]
    pub(crate) target_rss_sampled_kb: Option<u64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SummaryJson {
    pub(crate) suite: String,
    pub(crate) total_cases: usize,
    pub(crate) passed_cases: usize,
    pub(crate) failed_cases: usize,
    pub(crate) skipped_cases: usize,
    /// The run's own elapsed time from its run provenance; `null` when the
    /// run recorded none (historical or local reports).
    pub(crate) elapsed_ns: Option<u128>,
    pub(crate) measured_samples: usize,
    pub(crate) warmup_samples: usize,
    pub(crate) ranked_cases: usize,
    pub(crate) repetitions: usize,
    pub(crate) warmup: usize,
    /// What one latency sample times (`latency::MEASUREMENT_BOUNDARY`).
    pub(crate) measurement_boundary: String,
    /// Column layout of ranked.csv (`latency::RANKED_CSV_SCHEMA`).
    pub(crate) ranked_schema: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ManifestJson {
    pub(crate) schema_version: String,
    pub(crate) suite: String,
    pub(crate) command_line: Vec<String>,
    pub(crate) repetitions: usize,
    pub(crate) warmup: usize,
    pub(crate) output_files: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub(crate) struct EvidenceVersions {
    pub(crate) runner_version: String,
    pub(crate) target_version: String,
    pub(crate) sqlite_version: String,
    /// How the timed run was launched, e.g. "`run --suite all --workers auto`, workers=128".
    pub(crate) lane: String,
}

/// What the SQLite badge and report block may claim about one `sqlite_parity`
/// run: the corpus, the oracle and the counts, and whether validated official
/// evidence stands behind them. It never describes SQLite compatibility beyond
/// the SQL/CLI scripts in that corpus.
#[derive(Debug, Clone)]
pub(crate) struct SqliteQualification {
    /// The surface the corpus exercises: SQL and dot commands through the CLI.
    pub(crate) surface: &'static str,
    /// The corpus name; its cases are compiled into the runner that ran it.
    pub(crate) corpus_id: String,
    /// Runner release and binary SHA-256 recorded by the run, which pin the
    /// embedded corpus when the run recorded no corpus hash.
    pub(crate) runner_version: Option<String>,
    pub(crate) runner_sha256: Option<String>,
    /// `corpus_sha256` from the run evidence, when the run recorded it.
    pub(crate) corpus_sha256: Option<String>,
    /// SQLite release of the reference shell, e.g. `3.53.1`.
    pub(crate) oracle_version: Option<String>,
    /// Reference build stamp (`oracle_build_stamp`: archive SHA3-256 and
    /// compile flags from scripts/sqlite/build-reference.sh), when recorded.
    pub(crate) oracle_build_id: Option<String>,
    pub(crate) oracle_binary_sha256: Option<String>,
    /// SHA-256 of the run's official-evidence.json.
    pub(crate) run_id: Option<String>,
    pub(crate) total: usize,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) skipped: usize,
    /// Declared deviations among the cases in this run.
    pub(crate) deviation_count: usize,
    pub(crate) declared_deviations: Vec<DeclaredDeviation>,
    pub(crate) qualification: Qualification,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Qualification {
    /// Official evidence is present, bound to the raw results, and records
    /// the same counts and a SQLite oracle version.
    Qualified,
    /// Why the counts are not backed by official evidence.
    Unqualified(String),
}

/// A case whose pass does not mean RedlineDB matches the SQLite feature it
/// names (metadata/sqlite_parity/declared-deviations.json).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DeclaredDeviation {
    pub(crate) case_id: String,
    pub(crate) name: String,
    pub(crate) kind: DeviationKind,
    pub(crate) reason: String,
}

/// Why a declared case's pass says less than its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeviationKind {
    /// RedlineDB produces the compared output without the feature behind it.
    StandIn,
    /// The pinned SQLite build lacks the feature; the case declares its
    /// error and passes only when RedlineDB rejects the statement with that
    /// declared error.
    SharedRejection,
    /// The case was written for another SQLite build; against the pinned
    /// build it checks what the reason states.
    OracleBuild,
}

#[derive(Debug)]
pub(crate) struct RankedCase {
    pub(crate) case_id: String,
    pub(crate) name: String,
    pub(crate) case_file: String,
    pub(crate) priority: String,
    pub(crate) profile: String,
    pub(crate) category: String,
    pub(crate) sqlite_median_ns: u128,
    pub(crate) redline_median_ns: u128,
    /// RedlineDB median / SQLite median; lower is better.
    pub(crate) latency_ratio: f64,
    /// `(sqlite - redline) / sqlite * 100` with no reference floor.
    pub(crate) gap_pct: f64,
    /// The SQLite median is under `latency::RESOLUTION_NS`.
    pub(crate) below_resolution: bool,
    /// RedlineDB's median is strictly lower than SQLite's.
    pub(crate) faster: bool,
    pub(crate) samples: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct SvgArtifact {
    pub(crate) path: PathBuf,
    pub(crate) contents: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SvgSpec {
    pub(crate) title: String,
    pub(crate) subtitle: String,
    pub(crate) accent: &'static str,
    pub(crate) metrics: Vec<SvgMetric>,
    pub(crate) bars: Vec<SvgBar>,
}

#[derive(Debug, Clone)]
pub(crate) struct SvgMetric {
    pub(crate) label: String,
    pub(crate) value: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SvgBar {
    pub(crate) label: String,
    pub(crate) value: f64,
    pub(crate) value_label: String,
}

pub(crate) struct ArtifactNames {
    pub(crate) raw: &'static str,
    pub(crate) ranked: &'static str,
    pub(crate) summary: &'static str,
    pub(crate) manifest: &'static str,
    /// The report's own provenance. The run's provenance keeps its own
    /// name (`provenance.json`, staged as `run-provenance.json`).
    pub(crate) report_provenance: &'static str,
}

pub(crate) struct RenderedReport {
    pub(crate) raw: String,
    pub(crate) summary: String,
    pub(crate) ranked: String,
    pub(crate) readme: String,
    pub(crate) manifest: String,
    pub(crate) report_provenance: String,
}

pub(crate) struct Score {
    pub(crate) score: u64,
    pub(crate) status: String,
}
