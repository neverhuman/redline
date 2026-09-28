//! `summarize-bundle`: the statistics of a named release bench bundle
//! (L-05), re-derived from the bundle's own files.
//!
//! scripts/perf/release-bench.sh times N labelled RedlineDB binaries
//! against one SQLite reference with one runner, K interleaved runs each,
//! and writes:
//!
//! ```text
//! bundle.json                  what ran (redline-release-bench-bundle-v1)
//! host.json                    the host and each run's load (redline-release-bench-host-v1)
//! cases.json                   the runner's corpus listing, narrowed to a case list if any
//! cohorts/medium-set.txt       the medium cohort, when given
//! <label>/build-contract.json  the measured binaries and the declared build
//! <label>/build.json           scripts/perf/build-version.sh's record, when it built the binary
//! <label>/run-<k>/raw.jsonl    the runner's records, completion marker and evidence
//! ```
//!
//! Nothing is taken on trust. Every run must be the complete requested
//! experiment (`validate_run`), its records must name the label's binary and
//! the bundle's one reference, every label's contract must name the same
//! reference and runner, and every run must have been accepted under the
//! bundle's load threshold. The case is the unit (`summary::classify_cases`):
//! a label passed a case when every row of it passed in every run, and the
//! ratios are compared on the common pass set, the cases every label passed
//! in every run. The summary lists what keeps it from being publishable
//! (a narrowed corpus, fewer than 3 runs, an undeclared or mismatched
//! build, ...) rather than guessing.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::bundle_stats::{CohortStats, cohort_stats, delta};
use super::records::parse_rows;
use super::summary::{CaseOutcome, SummaryOptions, classify_cases};
use super::validate_run::{RunPlan, read_case_manifest, validate_run_path};

pub const BUNDLE_SCHEMA: &str = "redline-release-bench-bundle-v1";
pub const HOST_SCHEMA: &str = "redline-release-bench-host-v1";
pub const VERSION_BUILD_SCHEMA: &str = "redline-version-build-v1";
pub const BUNDLE_SUMMARY_SCHEMA: &str = "redline-release-bench-summary-v1";

pub const BUNDLE_ESTIMATOR: &str = "per case: RedlineDB median / SQLite median of the measured repetitions' elapsed ns (lower is better); per run: the median and nearest-rank p95 of those case ratios over the cohort; per label: the median of its runs' values, with their min-max";
pub const NOISE_RULE: &str = "a label's median change against the previous label is claimed only when the two labels' min-max ranges across runs do not overlap (exceeds_noise); overlapping ranges are within_noise; with fewer than two runs there is no spread (unassessed)";
pub const COMMON_PASS_SET: &str = "the cases every label passed in every run: every row passed, with exactly the warmups and measured repetitions the protocol asked for";

/// The fewest runs per label a publishable bundle has (R4-07).
pub const MIN_PUBLISHABLE_RUNS: usize = 3;

/// How every run of the bundle was measured.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Protocol {
    pub suite: String,
    pub workers: usize,
    pub repetitions: usize,
    pub warmup: usize,
    pub order: String,
    /// The taskset CPU list every run was pinned to; null when unpinned.
    pub cpus: Option<String>,
    pub tmp_root: String,
    /// `normal`: every run set REDLINEDB_DEFAULT_DURABILITY=normal (and
    /// REDLINEDB_QUIET_DURABILITY=1); `default`: neither was set, so each
    /// binary ran at its built-in default durability.
    pub durability: String,
    pub measurement_boundary: String,
    pub max_loadavg: f64,
    pub command: String,
}

/// A file inside the bundle, by its path relative to the bundle.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileRef {
    pub path: String,
    pub sha256: String,
    /// Where the bundle's copy came from.
    pub source: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleManifest {
    schema_version: String,
    bundle: String,
    started_at_utc: String,
    finished_at_utc: String,
    runs_per_label: usize,
    protocol: Protocol,
    cases: CaseSet,
    medium_cohort: Option<FileRef>,
    labels: Vec<LabelEntry>,
    runs: Vec<RunEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseSet {
    manifest: String,
    count: usize,
    case_list: Option<FileRef>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LabelEntry {
    label: String,
    binary: String,
    binary_sha256: String,
    source_ref: Option<String>,
    source_commit: Option<String>,
    build_contract: String,
    build_record: Option<String>,
    /// Whether the binary has the REDLINEDB_DEFAULT_DURABILITY knob (v4.0.9
    /// and older do not, and always run their built-in Strict default).
    durability_env: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunEntry {
    sequence: usize,
    label: String,
    run: usize,
    raw: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostRecord {
    schema_version: String,
    hostname: String,
    kernel: String,
    cpu_model: String,
    nproc: usize,
    pinned_cpus: Option<String>,
    /// `stat -f -c %T` of the directory the runs' temp roots were made in.
    tmp_filesystem: String,
    governors: BTreeMap<String, String>,
    runner_units_active: Vec<String>,
    max_loadavg: f64,
    runs: Vec<HostRun>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostRun {
    sequence: usize,
    label: String,
    run: usize,
    started_at_utc: String,
    finished_at_utc: String,
    loadavg_before: [f64; 3],
    loadavg_after: [f64; 3],
    runner_jobs_before: usize,
    runner_jobs_after: usize,
    runner_exit: i32,
    accepted: bool,
    reason: Option<String>,
}

/// The parts of build-contract.json (BM3-05) a bundle relies on.
#[derive(Debug, Deserialize)]
struct ContractView {
    schema_version: String,
    target: TargetView,
    reference: Identity,
    runner: Identity,
    optimization: OptimizationView,
}

#[derive(Debug, Deserialize)]
struct TargetView {
    sha256: String,
    version: String,
    build: DeclaredBuildView,
}

#[derive(Debug, Deserialize)]
struct OptimizationView {
    pgo_training_corpus: Option<String>,
}

/// A binary as a build contract names it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Identity {
    pub path: String,
    pub sha256: String,
    pub version: String,
}

/// A target's build as its builder declared it in the contract.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DeclaredBuildView {
    pub declared: bool,
    pub profile: Option<String>,
    pub features: Option<String>,
    pub rustflags: Option<String>,
}

/// The parts of scripts/perf/build-version.sh's build.json a bundle uses.
#[derive(Debug, Deserialize)]
struct VersionBuild {
    schema_version: String,
    source_commit: String,
    binary_sha256: String,
    rustc_verbose_version: Vec<String>,
    profile: String,
    rustflags: String,
    pgo: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BundleSummary {
    pub schema_version: &'static str,
    pub bundle: String,
    pub started_at_utc: String,
    pub finished_at_utc: String,
    pub estimator: &'static str,
    pub noise_rule: &'static str,
    pub protocol: Protocol,
    pub host: HostDigest,
    pub runner: Identity,
    pub reference: Identity,
    pub corpus: CorpusDigest,
    pub runs_per_label: usize,
    pub common_pass_set: CommonPassSet,
    pub medium_cohort: Option<MediumDigest>,
    pub labels: Vec<LabelSummary>,
    /// True only when `publication_blockers` is empty.
    pub publishable: bool,
    pub publication_blockers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HostDigest {
    pub cpu_model: String,
    pub nproc: usize,
    pub kernel: String,
    pub pinned_cpus: Option<String>,
    /// The filesystem the runs' temp roots were on; `tmpfs` to publish.
    pub tmp_filesystem: String,
    pub governors: BTreeMap<String, String>,
    pub runner_units_active: Vec<String>,
    /// Runs during which a CI runner job was active before or after.
    pub runs_with_runner_jobs: usize,
    pub max_loadavg_threshold: f64,
    /// The highest 1-minute load average recorded before or after a run.
    pub max_loadavg_observed: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CorpusDigest {
    /// The cases every run was asked to measure.
    pub cases: usize,
    /// The case list that narrowed the runner's corpus, if one did.
    pub narrowed_by: Option<FileRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CommonPassSet {
    pub definition: &'static str,
    pub cases: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MediumDigest {
    pub file: FileRef,
    pub listed: usize,
    pub in_bundle: usize,
    pub in_common_pass_set: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LabelSummary {
    pub label: String,
    pub source_ref: Option<String>,
    pub source_commit: Option<String>,
    pub binary_sha256: String,
    pub version: String,
    pub build: DeclaredBuildView,
    /// The durability the binary ran at: `normal` when the protocol set
    /// REDLINEDB_DEFAULT_DURABILITY=normal and the binary honors it,
    /// otherwise `built-in default`.
    pub durability: String,
    /// The first line of `rustc -vV` that built the binary, from its
    /// build.json; null when the bundle has no build record.
    pub build_rustc: Option<String>,
    pub pgo_training_corpus: Option<String>,
    /// Cases the label passed in every run.
    pub passed_cases: usize,
    /// Cases the label passed in some runs but not all.
    pub flaky_cases: usize,
    pub runs: Vec<RunDigest>,
    pub common: CohortStats,
    pub medium: Option<CohortStats>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunDigest {
    pub run: usize,
    pub sequence: usize,
    pub raw: String,
    pub raw_sha256: String,
    pub passed: usize,
    /// Cases with any failed row, the not-run rows included.
    pub failed: usize,
    pub skipped: usize,
    /// The runner's exit status; non-zero when a case failed.
    pub runner_exit: i32,
    /// 1-minute load averages before and after the run.
    pub loadavg_before: f64,
    pub loadavg_after: f64,
    /// Active CI runner jobs before and after the run.
    pub runner_jobs_before: usize,
    pub runner_jobs_after: usize,
}

/// The summary a bundle's files support, or the first reason they support
/// none.
pub fn summarize_bundle(dir: &Path) -> Result<BundleSummary> {
    let manifest: BundleManifest = read_json(&dir.join("bundle.json"))?;
    if manifest.schema_version != BUNDLE_SCHEMA {
        bail!(
            "bundle.json schema {:?} is not {BUNDLE_SCHEMA}",
            manifest.schema_version
        );
    }
    check_protocol(&manifest)?;
    let host: HostRecord = read_json(&dir.join("host.json"))?;
    let host_digest = check_host(&host, &manifest)?;
    let host_runs = host
        .runs
        .iter()
        .map(|run| ((run.label.as_str(), run.run), run))
        .collect::<BTreeMap<_, _>>();

    let case_ids = read_case_manifest(&bundle_path(dir, &manifest.cases.manifest)?)?;
    if case_ids.len() != manifest.cases.count {
        bail!(
            "cases.count is {}, but {} lists {} cases",
            manifest.cases.count,
            manifest.cases.manifest,
            case_ids.len()
        );
    }
    if let Some(case_list) = &manifest.cases.case_list {
        let listed = read_case_list(dir, case_list)?;
        if listed != case_ids {
            bail!(
                "the case list {} names {} cases, not the {} cases the runner listed",
                case_list.path,
                listed.len(),
                case_ids.len()
            );
        }
    }
    let medium = manifest
        .medium_cohort
        .as_ref()
        .map(|file| read_case_list(dir, file).map(|ids| (file, ids)))
        .transpose()?;
    if let Some((file, ids)) = &medium
        && manifest.cases.case_list.is_none()
        && let Some(missing) = ids.difference(&case_ids).next()
    {
        bail!(
            "medium cohort {} lists case {missing}, which the corpus does not have",
            file.path
        );
    }

    let plan = RunPlan {
        expected_cases: manifest.cases.count,
        repetitions: manifest.protocol.repetitions,
        warmup: manifest.protocol.warmup,
        case_manifest: Some(case_ids.clone()),
    };
    let mut labels = Vec::with_capacity(manifest.labels.len());
    let mut shared = None::<(Identity, Identity)>;
    for entry in &manifest.labels {
        let label = load_label(dir, entry, &manifest, &plan, &host_runs)
            .with_context(|| format!("label {}", entry.label))?;
        match &shared {
            None => shared = Some((label.reference.clone(), label.runner.clone())),
            Some((reference, runner)) => {
                if reference.sha256 != label.reference.sha256 {
                    bail!(
                        "label {} was timed against reference {}, not the bundle's {}",
                        entry.label,
                        label.reference.sha256,
                        reference.sha256
                    );
                }
                if runner.sha256 != label.runner.sha256 {
                    bail!(
                        "label {} was run by runner {}, not the bundle's {}",
                        entry.label,
                        label.runner.sha256,
                        runner.sha256
                    );
                }
            }
        }
        labels.push(label);
    }
    let Some((reference, runner)) = shared else {
        bail!("bundle.json names no label");
    };
    for label in &labels {
        for (run, table) in label.runs.iter().zip(&label.tables) {
            check_run_identities(table, &label.binary_sha256, &reference.sha256)
                .with_context(|| format!("label {} run {}", label.label, run.run))?;
        }
    }

    let common = labels
        .iter()
        .map(|label| label.passed.clone())
        .reduce(|left, right| left.intersection(&right).cloned().collect())
        .unwrap_or_default();
    if common.is_empty() {
        bail!("no case passed in every run of every label: the labels share nothing to compare");
    }
    let medium_common = medium
        .as_ref()
        .map(|(_, ids)| ids.intersection(&common).cloned().collect::<BTreeSet<_>>());

    let mut summaries = Vec::<LabelSummary>::with_capacity(labels.len());
    for label in labels {
        let tables = label
            .tables
            .iter()
            .map(|table| &table.cases)
            .collect::<Vec<_>>();
        let mut common_stats = cohort_stats(&tables, &common)?
            .with_context(|| format!("label {}: no common-pass-set ratios", label.label))?;
        let mut medium_stats = match &medium_common {
            Some(cohort) => cohort_stats(&tables, cohort)?,
            None => None,
        };
        if let Some(previous) = summaries.last() {
            common_stats.delta_vs_previous =
                Some(delta(&previous.label, &previous.common, &common_stats));
            if let (Some(previous_medium), Some(current)) = (&previous.medium, &mut medium_stats) {
                current.delta_vs_previous = Some(delta(&previous.label, previous_medium, current));
            }
        }
        summaries.push(LabelSummary {
            label: label.label,
            source_ref: label.source_ref,
            source_commit: label.source_commit,
            binary_sha256: label.binary_sha256,
            version: label.version,
            build: label.build,
            durability: label.durability,
            build_rustc: label.build_rustc,
            pgo_training_corpus: label.pgo_training_corpus,
            passed_cases: label.passed.len(),
            flaky_cases: label.flaky,
            runs: label.runs,
            common: common_stats,
            medium: medium_stats,
        });
    }

    let medium_digest = medium.map(|(file, ids)| MediumDigest {
        file: file.clone(),
        listed: ids.len(),
        in_bundle: ids.intersection(&case_ids).count(),
        in_common_pass_set: medium_common.as_ref().map_or(0, BTreeSet::len),
    });
    let mut summary = BundleSummary {
        schema_version: BUNDLE_SUMMARY_SCHEMA,
        bundle: manifest.bundle,
        started_at_utc: manifest.started_at_utc,
        finished_at_utc: manifest.finished_at_utc,
        estimator: BUNDLE_ESTIMATOR,
        noise_rule: NOISE_RULE,
        protocol: manifest.protocol,
        host: host_digest,
        runner,
        reference,
        corpus: CorpusDigest {
            cases: manifest.cases.count,
            narrowed_by: manifest.cases.case_list,
        },
        runs_per_label: manifest.runs_per_label,
        common_pass_set: CommonPassSet {
            definition: COMMON_PASS_SET,
            cases: common.len(),
        },
        medium_cohort: medium_digest,
        labels: summaries,
        publishable: false,
        publication_blockers: Vec::new(),
    };
    summary.publication_blockers = publication_blockers(&summary);
    summary.publishable = summary.publication_blockers.is_empty();
    Ok(summary)
}

/// The summary as summary.json holds it.
pub fn render_bundle_summary(summary: &BundleSummary) -> Result<String> {
    Ok(format!("{}\n", serde_json::to_string_pretty(summary)?))
}

/// Writes `<dir>/summary.json`, or with `check` fails unless it already
/// holds exactly what the bundle's files support.
pub fn write_bundle_summary(dir: &Path, check: bool) -> Result<BundleSummary> {
    let summary = summarize_bundle(dir)?;
    let text = render_bundle_summary(&summary)?;
    let path = dir.join("summary.json");
    if check {
        let existing =
            fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        if existing != text {
            bail!(
                "{} is not what the bundle's files support; rerun perf_evidence summarize-bundle",
                path.display()
            );
        }
    } else {
        fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    }
    Ok(summary)
}

/// A label with its runs loaded and checked.
struct LoadedLabel {
    label: String,
    source_ref: Option<String>,
    source_commit: Option<String>,
    binary_sha256: String,
    version: String,
    build: DeclaredBuildView,
    durability: String,
    build_rustc: Option<String>,
    pgo_training_corpus: Option<String>,
    reference: Identity,
    runner: Identity,
    runs: Vec<RunDigest>,
    tables: Vec<RunTable>,
    passed: BTreeSet<String>,
    flaky: usize,
}

struct RunTable {
    cases: BTreeMap<String, CaseOutcome>,
    target_digests: BTreeSet<String>,
    reference_digests: BTreeSet<String>,
}

fn load_label(
    dir: &Path,
    entry: &LabelEntry,
    manifest: &BundleManifest,
    plan: &RunPlan,
    host_runs: &BTreeMap<(&str, usize), &HostRun>,
) -> Result<LoadedLabel> {
    let contract: ContractView = read_json(&bundle_path(dir, &entry.build_contract)?)?;
    if contract.schema_version != super::BUILD_CONTRACT_SCHEMA {
        bail!(
            "{} schema {:?} is not {}",
            entry.build_contract,
            contract.schema_version,
            super::BUILD_CONTRACT_SCHEMA
        );
    }
    if contract.target.sha256 != entry.binary_sha256 {
        bail!(
            "{} names target {}, but bundle.json records {} for {}",
            entry.build_contract,
            contract.target.sha256,
            entry.binary_sha256,
            entry.binary
        );
    }
    let build_rustc = match &entry.build_record {
        Some(path) => Some(check_build_record(dir, path, entry, &contract)?),
        None => None,
    };

    let mut runs = manifest
        .runs
        .iter()
        .filter(|run| run.label == entry.label)
        .collect::<Vec<_>>();
    runs.sort_by_key(|run| run.run);
    let numbers = runs.iter().map(|run| run.run).collect::<Vec<_>>();
    if numbers != (1..=manifest.runs_per_label).collect::<Vec<_>>() {
        bail!(
            "runs {numbers:?} are not 1..={} exactly once",
            manifest.runs_per_label
        );
    }
    let options = SummaryOptions {
        expected_repetitions: Some(plan.repetitions),
    };
    let mut digests = Vec::with_capacity(runs.len());
    let mut tables = Vec::with_capacity(runs.len());
    for run in runs {
        let raw = bundle_path(dir, &run.raw)?;
        let validation = validate_run_path(&raw, plan)
            .with_context(|| format!("run {} ({})", run.run, run.raw))?;
        let file = File::open(&raw).with_context(|| format!("open {}", raw.display()))?;
        let rows =
            parse_rows(BufReader::new(file)).with_context(|| format!("parse {}", raw.display()))?;
        let cases = classify_cases(&rows, options)
            .with_context(|| format!("run {} ({})", run.run, run.raw))?
            .cases;
        let passed = cases
            .values()
            .filter(|outcome| matches!(outcome, CaseOutcome::Eligible(_)))
            .count();
        let host = host_runs
            .get(&(entry.label.as_str(), run.run))
            .with_context(|| format!("host.json has no run {}", run.run))?;
        if host.runner_exit != 0 && validation.failed_cases == 0 {
            bail!(
                "run {}: the runner exited {} although no case failed",
                run.run,
                host.runner_exit
            );
        }
        digests.push(RunDigest {
            run: run.run,
            sequence: run.sequence,
            raw: run.raw.clone(),
            raw_sha256: validation.raw_sha256,
            passed,
            failed: validation.failed_cases,
            skipped: validation.skipped_cases,
            runner_exit: host.runner_exit,
            loadavg_before: host.loadavg_before[0],
            loadavg_after: host.loadavg_after[0],
            runner_jobs_before: host.runner_jobs_before,
            runner_jobs_after: host.runner_jobs_after,
        });
        tables.push(RunTable {
            cases,
            target_digests: rows
                .iter()
                .filter_map(|row| row.target_sha256.clone())
                .collect(),
            reference_digests: rows
                .iter()
                .filter_map(|row| row.reference_sha256.clone())
                .collect(),
        });
    }

    let passed_per_run = tables
        .iter()
        .map(|table| {
            table
                .cases
                .iter()
                .filter(|(_, outcome)| matches!(outcome, CaseOutcome::Eligible(_)))
                .map(|(case_id, _)| case_id.clone())
                .collect::<BTreeSet<_>>()
        })
        .collect::<Vec<_>>();
    let passed = passed_per_run
        .iter()
        .cloned()
        .reduce(|left, right| left.intersection(&right).cloned().collect())
        .unwrap_or_default();
    let ever = passed_per_run
        .iter()
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>();
    Ok(LoadedLabel {
        label: entry.label.clone(),
        source_ref: entry.source_ref.clone(),
        source_commit: entry.source_commit.clone(),
        binary_sha256: entry.binary_sha256.clone(),
        version: contract.target.version,
        build: contract.target.build,
        durability: if manifest.protocol.durability == "normal" && entry.durability_env {
            "normal".to_owned()
        } else {
            "built-in default".to_owned()
        },
        build_rustc,
        pgo_training_corpus: contract.optimization.pgo_training_corpus,
        reference: contract.reference,
        runner: contract.runner,
        runs: digests,
        tables,
        flaky: ever.len() - passed.len(),
        passed,
    })
}

/// The records of a run must name exactly the label's binary and the
/// bundle's reference: a raw file copied from another label or another
/// bundle is refused.
fn check_run_identities(table: &RunTable, target: &str, reference: &str) -> Result<()> {
    for (role, found, expected) in [
        ("target", &table.target_digests, target),
        ("reference", &table.reference_digests, reference),
    ] {
        if found.len() != 1 || !found.contains(expected) {
            bail!("its records name {role} executables {found:?}, not exactly {expected}",);
        }
    }
    Ok(())
}

/// build.json must describe this binary and agree with the contract's
/// declared build; returns the first line of its `rustc -vV`.
fn check_build_record(
    dir: &Path,
    path: &str,
    entry: &LabelEntry,
    contract: &ContractView,
) -> Result<String> {
    let record: VersionBuild = read_json(&bundle_path(dir, path)?)?;
    if record.schema_version != VERSION_BUILD_SCHEMA {
        bail!(
            "{path} schema {:?} is not {VERSION_BUILD_SCHEMA}",
            record.schema_version
        );
    }
    if record.binary_sha256 != entry.binary_sha256 {
        bail!(
            "{path} built {}, not the measured {}",
            record.binary_sha256,
            entry.binary_sha256
        );
    }
    if entry.source_commit.as_deref() != Some(record.source_commit.as_str()) {
        bail!(
            "{path} built {}, but bundle.json records source commit {:?}",
            record.source_commit,
            entry.source_commit
        );
    }
    let build = &contract.target.build;
    let declared = DeclaredBuildView {
        declared: true,
        profile: Some(record.profile.clone()),
        features: build.features.clone(),
        rustflags: Some(record.rustflags.clone()),
    };
    if !build.declared || build.profile != declared.profile || build.rustflags != declared.rustflags
    {
        bail!("{path} records {declared:?}, but the build contract declares {build:?}");
    }
    if record.pgo != contract.optimization.pgo_training_corpus.is_some() {
        bail!(
            "{path} records pgo {}, the build contract a training corpus of {:?}",
            record.pgo,
            contract.optimization.pgo_training_corpus
        );
    }
    record
        .rustc_verbose_version
        .first()
        .cloned()
        .with_context(|| format!("{path} records no rustc -vV output"))
}

fn check_protocol(manifest: &BundleManifest) -> Result<()> {
    let protocol = &manifest.protocol;
    if protocol.suite != "sqlite_parity" {
        bail!(
            "the protocol's suite is {:?}, not sqlite_parity",
            protocol.suite
        );
    }
    if !matches!(protocol.durability.as_str(), "normal" | "default") {
        bail!(
            "the protocol's durability is {:?}, not normal or default",
            protocol.durability
        );
    }
    if protocol.repetitions == 0 || protocol.workers == 0 {
        bail!("the protocol needs at least one worker and one measured repetition");
    }
    if manifest.runs_per_label == 0 {
        bail!("runs_per_label must be positive");
    }
    if !(protocol.max_loadavg.is_finite() && protocol.max_loadavg > 0.0) {
        bail!(
            "max_loadavg {} is not a positive number",
            protocol.max_loadavg
        );
    }
    let mut labels = BTreeSet::new();
    for entry in &manifest.labels {
        if entry.label.is_empty() || !labels.insert(entry.label.as_str()) {
            bail!("label {:?} is empty or repeated", entry.label);
        }
    }
    let mut sequences = BTreeSet::new();
    let mut raws = BTreeSet::new();
    for run in &manifest.runs {
        if !labels.contains(run.label.as_str()) {
            bail!("run {} names unknown label {}", run.sequence, run.label);
        }
        if !sequences.insert(run.sequence) || !raws.insert(run.raw.as_str()) {
            bail!(
                "run sequence {} or raw {} is used twice",
                run.sequence,
                run.raw
            );
        }
    }
    Ok(())
}

/// Every run must have been accepted under the bundle's load threshold,
/// and host.json must describe exactly the bundle's runs.
fn check_host(host: &HostRecord, manifest: &BundleManifest) -> Result<HostDigest> {
    if host.schema_version != HOST_SCHEMA {
        bail!(
            "host.json schema {:?} is not {HOST_SCHEMA}",
            host.schema_version
        );
    }
    let protocol = &manifest.protocol;
    if host.max_loadavg != protocol.max_loadavg || host.pinned_cpus != protocol.cpus {
        bail!(
            "host.json records threshold {} and CPUs {:?}, the protocol {} and {:?}",
            host.max_loadavg,
            host.pinned_cpus,
            protocol.max_loadavg,
            protocol.cpus
        );
    }
    let recorded = host
        .runs
        .iter()
        .map(|run| (run.sequence, run.label.as_str(), run.run))
        .collect::<BTreeSet<_>>();
    let expected = manifest
        .runs
        .iter()
        .map(|run| (run.sequence, run.label.as_str(), run.run))
        .collect::<BTreeSet<_>>();
    if recorded != expected || host.runs.len() != manifest.runs.len() {
        bail!("host.json records runs {recorded:?}, bundle.json {expected:?}");
    }
    let mut observed = 0.0_f64;
    let mut with_jobs = 0;
    for run in &host.runs {
        let label = format!(
            "host.json run {} ({} run {})",
            run.sequence, run.label, run.run
        );
        if !run.accepted {
            bail!(
                "{label} was rejected: {}",
                run.reason.as_deref().unwrap_or("no reason")
            );
        }
        for load in [run.loadavg_before[0], run.loadavg_after[0]] {
            if !load.is_finite() || load > host.max_loadavg {
                bail!(
                    "{label}: 1-minute load average {load} exceeds the threshold {}",
                    host.max_loadavg
                );
            }
            observed = observed.max(load);
        }
        if run.started_at_utc.is_empty() || run.finished_at_utc.is_empty() {
            bail!("{label} records no start or finish time");
        }
        with_jobs += usize::from(run.runner_jobs_before + run.runner_jobs_after > 0);
    }
    if host.hostname.is_empty() {
        bail!("host.json records no hostname");
    }
    Ok(HostDigest {
        cpu_model: host.cpu_model.clone(),
        nproc: host.nproc,
        kernel: host.kernel.clone(),
        pinned_cpus: host.pinned_cpus.clone(),
        tmp_filesystem: host.tmp_filesystem.clone(),
        governors: host.governors.clone(),
        runner_units_active: host.runner_units_active.clone(),
        runs_with_runner_jobs: with_jobs,
        max_loadavg_threshold: host.max_loadavg,
        max_loadavg_observed: observed,
    })
}

/// Why the bundle cannot back a public claim, if it cannot.
fn publication_blockers(summary: &BundleSummary) -> Vec<String> {
    let mut blockers = Vec::new();
    if let Some(case_list) = &summary.corpus.narrowed_by {
        blockers.push(format!(
            "the corpus is narrowed to {} cases by {}",
            summary.corpus.cases, case_list.path
        ));
    }
    if summary.runs_per_label < MIN_PUBLISHABLE_RUNS {
        blockers.push(format!(
            "{} run(s) per label; a spread needs at least {MIN_PUBLISHABLE_RUNS}",
            summary.runs_per_label
        ));
    }
    if summary.protocol.workers != 1 {
        blockers.push(format!(
            "{} workers; the timing lane runs one case at a time",
            summary.protocol.workers
        ));
    }
    if summary.protocol.cpus.is_none() {
        blockers.push("the runs were not pinned to CPUs".to_owned());
    }
    if summary.host.tmp_filesystem != "tmpfs" {
        blockers.push(format!(
            "the temp roots were on {}, not tmpfs",
            summary.host.tmp_filesystem
        ));
    }
    let mut rustflags = BTreeSet::new();
    for label in &summary.labels {
        let build = &label.build;
        if !build.declared {
            blockers.push(format!("{}: the build is undeclared", label.label));
        } else if build.profile.as_deref() != Some("release") {
            blockers.push(format!(
                "{}: built with profile {:?}, not release",
                label.label, build.profile
            ));
        }
        if label.pgo_training_corpus.is_some() {
            blockers.push(format!("{}: a PGO build", label.label));
        }
        if label.source_commit.is_none() {
            blockers.push(format!("{}: no source commit recorded", label.label));
        }
        if summary.protocol.durability == "normal" && label.durability != "normal" {
            blockers.push(format!(
                "{}: the binary has no REDLINEDB_DEFAULT_DURABILITY knob, so it ran at its built-in default durability, not normal",
                label.label
            ));
        }
        rustflags.insert(build.rustflags.clone());
    }
    if rustflags.len() > 1 {
        blockers.push(format!(
            "the labels were built with different RUSTFLAGS: {rustflags:?}"
        ));
    }
    blockers
}

/// A case list's ids (five digits, `#` comments allowed), after checking
/// the bundle's copy against its recorded SHA-256.
fn read_case_list(dir: &Path, file: &FileRef) -> Result<BTreeSet<String>> {
    let path = bundle_path(dir, &file.path)?;
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if digest != file.sha256 {
        bail!(
            "{} has SHA-256 {digest}, but the bundle records {}",
            file.path,
            file.sha256
        );
    }
    let text = String::from_utf8(bytes).with_context(|| format!("{} is not UTF-8", file.path))?;
    parse_case_list(&text).with_context(|| format!("case list {}", file.path))
}

/// Parses a case list: one numeric case id per line, `#` starting a
/// comment. Ids are returned as the five-digit display ids raw records
/// carry; a repeated id or an empty list is an error.
pub fn parse_case_list(text: &str) -> Result<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        let id = line.split('#').next().unwrap_or_default().trim();
        if id.is_empty() {
            continue;
        }
        if id.len() > 5 || !id.bytes().all(|byte| byte.is_ascii_digit()) {
            bail!("line {}: {id:?} is not a case id", index + 1);
        }
        let id = format!("{:0>5}", id);
        if !ids.insert(id.clone()) {
            bail!("line {}: case {id} is listed twice", index + 1);
        }
    }
    if ids.is_empty() {
        bail!("lists no case");
    }
    Ok(ids)
}

/// A path inside the bundle: relative, with no `..`, `.` or root.
fn bundle_path(dir: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    if relative.is_empty()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        bail!("{relative:?} is not a plain path inside the bundle");
    }
    Ok(dir.join(path))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

#[cfg(test)]
#[path = "bundle_tests.rs"]
mod tests;
