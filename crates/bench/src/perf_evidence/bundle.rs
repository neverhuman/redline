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

mod checks;
mod label;
mod manifest;
mod report;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::bundle_stats::{cohort_stats, delta};
use super::validate_run::{RunPlan, read_case_manifest};

use checks::{check_host, check_protocol};
use label::{check_run_identities, load_label};
pub use manifest::parse_case_list;
use manifest::{BundleManifest, HostRecord, Identity, bundle_path, read_case_list, read_json};
pub use report::{BundleSummary, render_bundle_summary, write_bundle_summary};
use report::{CommonPassSet, CorpusDigest, LabelSummary, MediumDigest, publication_blockers};

pub const BUNDLE_SCHEMA: &str = "redline-release-bench-bundle-v1";
pub const HOST_SCHEMA: &str = "redline-release-bench-host-v1";
pub const VERSION_BUILD_SCHEMA: &str = "redline-version-build-v1";
pub const BUNDLE_SUMMARY_SCHEMA: &str = "redline-release-bench-summary-v1";

pub const BUNDLE_ESTIMATOR: &str = "per case: RedlineDB median / SQLite median of the measured repetitions' elapsed ns (lower is better); per run: the median and nearest-rank p95 of those case ratios over the cohort; per label: the median of its runs' values, with their min-max";
pub const NOISE_RULE: &str = "a label's median change against the previous label is claimed only when the two labels' min-max ranges across runs do not overlap (exceeds_noise); overlapping ranges are within_noise; with fewer than two runs there is no spread (unassessed)";
pub const COMMON_PASS_SET: &str = "the cases every label passed in every run: every row passed, with exactly the warmups and measured repetitions the protocol asked for";

/// The fewest runs per label a publishable bundle has (R4-07).
pub const MIN_PUBLISHABLE_RUNS: usize = 3;

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

#[cfg(test)]
#[path = "bundle_tests.rs"]
mod tests;
