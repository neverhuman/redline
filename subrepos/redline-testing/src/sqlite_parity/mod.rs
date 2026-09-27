mod bounded;
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
#[cfg(test)]
mod target_contract_tests;
#[cfg(test)]
mod test_fixtures;
mod text;

use std::path::PathBuf;

use anyhow::Result;

pub use bounded::{Limits, check_kill};
pub use catalog::all_cases;
pub use engine::REFERENCE_CLI_BIN;
pub use identity::{assertion_policy_sha256, corpus_sha256};
pub use known_failures::{BaselineSource, KNOWN_FAILURES_SCHEMA, KnownFailures};
pub use record_sink::completion_marker_path;
pub use rql_phase1::rql_phase1_cases;
pub use runner::RunSummary;
#[cfg(test)]
pub(crate) use runner::{CaseFailure, VerdictReason};

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
}

pub fn run(config: RunConfig) -> Result<RunSummary> {
    let cases = catalog::narrow_to_ids(catalog::selected_official_cases()?, &config.case_ids)?;
    let reference = engine::EngineSpec::new(engine::REFERENCE_CLI_BIN, config.reference_bin)
        .with_limits(config.limits);
    let target = engine::EngineSpec::new("redlinedb", config.target_bin).with_limits(config.limits);
    runner::validate_compare_engines(&reference, &target)?;
    let capabilities = reference.sqlite_shell_capabilities()?;
    let sqlite_version = capabilities
        .as_ref()
        .map(|capabilities| capabilities.version.clone());
    // Probe the target binary too — cases that gate on optional SQLite
    // features (fts5, rtree, dbstat, …) are skipped when the target
    // lacks them, per the ship-contract: corpus ships what passes
    // reference self-compare; the parity sweep must not gate on target
    // engine readiness for explicitly-optional features.
    let target_capabilities = target.target_capabilities().ok();
    let partition =
        engine::partition_cases(cases, capabilities.as_ref(), target_capabilities.as_ref());
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
        sqlite_version,
        progress: config.progress,
        memory_samples: config.memory_samples,
    };
    runner::compare_cases(
        &run,
        &pairs,
        &partition.skipped,
        config.workers,
        record_sink::RecordSink::open(&config.output, config.suite)?,
    )
}

pub fn run_rql_phase1(config: RunConfig) -> Result<RunSummary> {
    rql_phase1::run(config)
}
