//! One label of a bundle: its build contract and build record checked, and
//! each of its runs validated, classified and matched to host.json.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::VERSION_BUILD_SCHEMA;
use super::manifest::{
    BundleManifest, ContractView, DeclaredBuildView, HostRun, Identity, LabelEntry, VersionBuild,
    bundle_path, read_json,
};
use super::report::RunDigest;
use crate::perf_evidence::records::parse_rows;
use crate::perf_evidence::summary::{CaseOutcome, SummaryOptions, classify_cases};
use crate::perf_evidence::validate_run::{RunPlan, validate_run_path};

/// A label with its runs loaded and checked.
pub(super) struct LoadedLabel {
    pub(super) label: String,
    pub(super) source_ref: Option<String>,
    pub(super) source_commit: Option<String>,
    pub(super) binary_sha256: String,
    pub(super) version: String,
    pub(super) build: DeclaredBuildView,
    pub(super) durability: String,
    pub(super) build_rustc: Option<String>,
    pub(super) pgo_training_corpus: Option<String>,
    pub(super) reference: Identity,
    pub(super) runner: Identity,
    pub(super) runs: Vec<RunDigest>,
    pub(super) tables: Vec<RunTable>,
    pub(super) passed: BTreeSet<String>,
    pub(super) flaky: usize,
}

pub(super) struct RunTable {
    pub(super) cases: BTreeMap<String, CaseOutcome>,
    pub(super) target_digests: BTreeSet<String>,
    pub(super) reference_digests: BTreeSet<String>,
}

pub(super) fn load_label(
    dir: &Path,
    entry: &LabelEntry,
    manifest: &BundleManifest,
    plan: &RunPlan,
    host_runs: &BTreeMap<(&str, usize), &HostRun>,
) -> Result<LoadedLabel> {
    let contract: ContractView = read_json(&bundle_path(dir, &entry.build_contract)?)?;
    if contract.schema_version != crate::perf_evidence::BUILD_CONTRACT_SCHEMA {
        bail!(
            "{} schema {:?} is not {}",
            entry.build_contract,
            contract.schema_version,
            crate::perf_evidence::BUILD_CONTRACT_SCHEMA
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
pub(super) fn check_run_identities(table: &RunTable, target: &str, reference: &str) -> Result<()> {
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
