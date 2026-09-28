mod bounded;
#[cfg(all(test, unix))]
mod capability_tests;
pub mod case;
mod catalog;
mod compare;
mod engine;
mod identity;
mod known_failures;
mod memory;
mod normalize;
mod record_sink;
mod report;
mod rql_phase1;
mod runner;
mod scope_policy;
#[cfg(test)]
mod target_contract_tests;
#[cfg(test)]
mod test_fixtures;
mod text;

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Result, bail};

pub use bounded::{Limits, check_kill};
pub use catalog::{all_cases, check_official_selection};
pub use engine::REFERENCE_CLI_BIN;
pub use identity::{assertion_policy_sha256, corpus_sha256};
pub use known_failures::{BaselineSource, KNOWN_FAILURES_SCHEMA, KnownFailures};
pub use record_sink::completion_marker_path;
pub use rql_phase1::rql_phase1_cases;
pub use runner::RunSummary;
#[cfg(test)]
pub(crate) use runner::{CaseFailure, VerdictReason};
pub use scope_policy::{SCOPE_POLICY_PATH, SCOPE_POLICY_SCHEMA, ScopePolicy, today_utc};

pub struct RunConfig {
    /// The suite the records belong to: `sqlite_parity`, `memory` or
    /// `rql_phase1`.
    pub suite: &'static str,
    pub reference_bin: PathBuf,
    pub target_bin: PathBuf,
    pub output: PathBuf,
    pub tmp_root: PathBuf,
    pub workers: usize,
    pub repetitions: usize,
    pub warmup: usize,
    pub progress: bool,
    pub memory_samples: bool,
    /// Only these case ids (a diagnostic run); every case when empty.
    pub case_ids: Vec<String>,
    /// The deadline and output cap of every engine run (SQ-09).
    pub limits: Limits,
    /// An official run: the whole corpus, no environment narrowing (SQ-05).
    pub official: bool,
}

pub fn run(config: RunConfig) -> Result<RunSummary> {
    let policy = ScopePolicy::compiled()?;
    let cases = catalog::narrow_to_ids(catalog::select_cases(config.official)?, &config.case_ids)?;
    let reference = engine::EngineSpec::new(engine::REFERENCE_CLI_BIN, config.reference_bin)
        .with_limits(config.limits);
    let target = engine::EngineSpec::new("redlinedb", config.target_bin).with_limits(config.limits);
    runner::validate_compare_engines(&reference, &target)?;
    // Cases that gate on an optional feature (fts5, rtree, dbstat, …) are
    // judged against what both shells can do. A probe that cannot run is
    // an error (SQ-05): it never turns into a skip or into no gating.
    let capabilities = reference.capabilities()?;
    let target_capabilities = target.capabilities()?;
    let partition = engine::partition_cases(
        cases,
        &capabilities,
        &target_capabilities,
        &policy,
        config.suite,
    )?;
    let pairs = partition
        .runnable
        .into_iter()
        .map(runner::CasePair::same)
        .collect::<Vec<_>>();
    let run = runner::SuiteRun {
        label: "sqlite_parity",
        reference: &reference,
        target: &target,
        tmp_root: &config.tmp_root,
        warmup: config.warmup,
        repetitions: config.repetitions,
        sqlite_version: Some(capabilities.version.clone()),
        progress: config.progress,
        memory_samples: config.memory_samples,
    };
    runner::compare_cases(
        &run,
        &pairs,
        &partition.skipped,
        &partition.rejected,
        config.workers,
        record_sink::RecordSink::open(&config.output, config.suite)?,
    )
}

/// The case ids every complete run of `suite` covers, whatever the
/// environment says: the manifest `report` and `check-sqlite` hold raw
/// records to (SQ-04).
pub fn manifest_case_ids(suite: &str) -> Result<BTreeSet<String>> {
    let cases = catalog::select_cases_with(false, None)?;
    Ok(match suite {
        "sqlite_parity" | "memory" => cases.iter().map(case::Case::display_id).collect(),
        "rql_phase1" => cases
            .iter()
            .filter(|case| rql_phase1::is_rql_phase1_source(case))
            .map(case::Case::display_id)
            .collect(),
        other => bail!("suite {other} has no SQLite case manifest"),
    })
}

pub fn run_rql_phase1(config: RunConfig) -> Result<RunSummary> {
    rql_phase1::run(config)
}
