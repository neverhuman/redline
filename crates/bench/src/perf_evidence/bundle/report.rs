//! The summary a bundle's files support, written as summary.json, and the
//! reasons it cannot back a public claim.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::manifest::{DeclaredBuildView, FileRef, Identity, Protocol};
use super::{MIN_PUBLISHABLE_RUNS, summarize_bundle};
use crate::perf_evidence::bundle_stats::CohortStats;

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

/// Why the bundle cannot back a public claim, if it cannot.
pub(super) fn publication_blockers(summary: &BundleSummary) -> Vec<String> {
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
