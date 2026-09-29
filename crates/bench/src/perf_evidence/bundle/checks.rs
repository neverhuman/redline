//! The bundle-wide checks: the protocol bundle.json declares, and host.json's
//! record of every run under the bundle's `max_loadavg`.

use std::collections::BTreeSet;

use anyhow::{Result, bail};

use super::HOST_SCHEMA;
use super::manifest::{BundleManifest, HostRecord};
use super::report::HostDigest;

pub(super) fn check_protocol(manifest: &BundleManifest) -> Result<()> {
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
pub(super) fn check_host(host: &HostRecord, manifest: &BundleManifest) -> Result<HostDigest> {
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
