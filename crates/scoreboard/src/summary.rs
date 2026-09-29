//! Summarize a bundle: the raw records of several versions, each measured
//! beside SQLite in the same runs, into medians, paired speedups and
//! publication blockers. `summary.json` is derived; the raw records are
//! the evidence, and `--check` recomputes the summary from them.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::workloads::CATALOG;

pub const SUMMARY_SCHEMA: &str = "redline-scoreboard-summary-v1";
pub const BUNDLE_SCHEMA: &str = "redline-scoreboard-bundle-v1";

/// Fewest runs of each version a publishable bundle holds.
pub const MIN_RUNS: usize = 3;
/// SQLite is the control group: its medians from different versions' runs
/// on the same host must agree this closely, or the host drifted.
pub const SQLITE_DRIFT: f64 = 0.10;

/// `bundle.json`, written by `scripts/perf/scoreboard-bench.sh`.
#[derive(Debug, Deserialize)]
pub struct BundleManifest {
    pub schema: String,
    pub bundle: String,
    /// Versions in order; the first is the baseline speedups divide by.
    pub labels: Vec<String>,
    pub rows: u64,
    pub runs: u32,
    pub pairs: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Summary {
    pub schema: String,
    pub bundle: String,
    pub labels: Vec<String>,
    pub rows: u64,
    pub runs: u32,
    pub publishable: bool,
    pub blockers: Vec<String>,
    pub sqlite_version: Option<String>,
    pub raw: Vec<RawFile>,
    pub workloads: Vec<WorkloadSummary>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct RawFile {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct WorkloadSummary {
    pub pair: String,
    pub id: String,
    pub unit: String,
    pub summary: String,
    /// Throughput of each version, then of SQLite under the key "sqlite".
    pub throughput: BTreeMap<String, Figure>,
    /// Each version's throughput over SQLite's in the same run.
    pub vs_sqlite: BTreeMap<String, Figure>,
    /// Each later version over the baseline, paired run by run.
    pub speedup: BTreeMap<String, Speedup>,
    pub digest_agrees: bool,
}

/// A median across runs of per-run medians, with the runs' range.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Figure {
    pub median: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Runs in which a repetition timed out; such a run has no figure.
    pub timed_out_runs: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Speedup {
    pub median: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// The two versions' run ranges do not overlap.
    pub exceeds_noise: bool,
}

#[derive(Debug, Deserialize)]
struct Record {
    status: String,
    label: String,
    run: u32,
    pair: String,
    workload: String,
    #[serde(default)]
    ops: u64,
    #[serde(default)]
    elapsed_ns: u64,
    #[serde(default)]
    digest: i64,
    #[serde(default)]
    work_divisor: u64,
    #[serde(default)]
    engine_version: String,
    /// The version whose run measured the record.
    #[serde(default)]
    with: String,
}

/// Every raw record file of the bundle: `<label>/run-<k>-<pair>.jsonl`.
fn raw_files(bundle: &Path, labels: &[String]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for label in labels {
        let dir = bundle.join(label);
        let mut found: Vec<PathBuf> = fs::read_dir(&dir)
            .with_context(|| format!("read {}", dir.display()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("run-") && name.ends_with(".jsonl"))
            })
            .collect();
        found.sort();
        files.extend(found);
    }
    Ok(files)
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let mid = values.len() / 2;
    Some(if values.len() % 2 == 1 {
        values[mid]
    } else {
        (values[mid - 1] + values[mid]) / 2.0
    })
}

fn figure(per_run: &BTreeMap<u32, f64>, timed_out_runs: u32) -> Figure {
    let mut values: Vec<f64> = per_run.values().copied().collect();
    let min = values.iter().copied().reduce(f64::min);
    let max = values.iter().copied().reduce(f64::max);
    Figure {
        median: median(&mut values),
        min,
        max,
        timed_out_runs,
    }
}

/// Compute the summary of the bundle at `bundle`.
pub fn summarize(bundle: &Path) -> Result<Summary> {
    let manifest: BundleManifest =
        serde_json::from_slice(&fs::read(bundle.join("bundle.json")).context("read bundle.json")?)
            .context("parse bundle.json")?;
    if manifest.schema != BUNDLE_SCHEMA {
        bail!(
            "bundle.json schema is {}, expected {BUNDLE_SCHEMA}",
            manifest.schema
        );
    }
    let files = raw_files(bundle, &manifest.labels)?;
    let mut raw = Vec::new();
    let mut records = Vec::new();
    for path in &files {
        let relative = path.strip_prefix(bundle)?.to_string_lossy().into_owned();
        raw.push(RawFile {
            path: relative.clone(),
            sha256: sha256_file(path)?,
        });
        let text = fs::read_to_string(path)?;
        for (number, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let record: Record =
                serde_json::from_str(line).with_context(|| format!("{relative}:{}", number + 1))?;
            records.push(record);
        }
    }

    let mut blockers = BTreeSet::new();
    let mut sqlite_version = None;
    // (pair, workload, label, run) -> throughputs of the ok repetitions
    let mut samples: BTreeMap<(String, String, String, u32), Vec<f64>> = BTreeMap::new();
    let mut timeouts: BTreeMap<(String, String, String), BTreeSet<u32>> = BTreeMap::new();
    let mut digests: BTreeMap<(String, String), BTreeSet<i64>> = BTreeMap::new();
    let mut runs_seen: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    for record in &records {
        runs_seen
            .entry(record.label.clone())
            .or_default()
            .insert(record.run);
        match record.status.as_str() {
            "ok" => {
                if record.work_divisor != 1 {
                    blockers.insert(format!(
                        "{} {}: measured with work divisor {}",
                        record.label, record.workload, record.work_divisor
                    ));
                }
                if record.label == "sqlite" && sqlite_version.is_none() {
                    sqlite_version = Some(record.engine_version.clone());
                }
                if record.elapsed_ns == 0 {
                    continue;
                }
                let throughput = record.ops as f64 * 1e9 / record.elapsed_ns as f64;
                let mut series = vec![record.label.clone()];
                if record.label == "sqlite" {
                    series.push(format!("sqlite@{}", record.with));
                }
                for series in series {
                    samples
                        .entry((
                            record.pair.clone(),
                            record.workload.clone(),
                            series,
                            record.run,
                        ))
                        .or_default()
                        .push(throughput);
                }
                digests
                    .entry((record.pair.clone(), record.workload.clone()))
                    .or_default()
                    .insert(record.digest);
            }
            "timeout" => {
                timeouts
                    .entry((
                        record.pair.clone(),
                        record.workload.clone(),
                        record.label.clone(),
                    ))
                    .or_default()
                    .insert(record.run);
            }
            other => {
                blockers.insert(format!(
                    "{} {} run {}: {other}",
                    record.label, record.workload, record.run
                ));
            }
        }
    }
    for label in &manifest.labels {
        let runs = runs_seen.get(label).map_or(0, BTreeSet::len);
        if runs < MIN_RUNS {
            blockers.insert(format!("{label}: {runs} runs, fewer than {MIN_RUNS}"));
        }
    }

    // SQLite is measured beside every version, keyed by that version's
    // label in the run files: `sqlite` records in `<label>/...` files.
    let mut workloads = Vec::new();
    for pair in &manifest.pairs {
        for entry in CATALOG {
            let key_of =
                |label: &str, run: u32| (pair.clone(), entry.id.to_owned(), label.to_owned(), run);
            let any = samples
                .keys()
                .any(|(p, w, _, _)| p == pair && w == entry.id);
            if !any {
                continue;
            }
            let mut throughput = BTreeMap::new();
            let mut vs_sqlite = BTreeMap::new();
            let mut per_label_runs: BTreeMap<String, BTreeMap<u32, f64>> = BTreeMap::new();
            for label in manifest
                .labels
                .iter()
                .chain(std::iter::once(&"sqlite".to_owned()))
            {
                let mut per_run = BTreeMap::new();
                for run in 1..=manifest.runs {
                    if let Some(values) = samples.get(&key_of(label, run)) {
                        let mut values = values.clone();
                        if let Some(m) = median(&mut values) {
                            per_run.insert(run, m);
                        }
                    }
                }
                let timed_out = timeouts
                    .get(&(pair.clone(), entry.id.to_owned(), label.clone()))
                    .map_or(0, |runs| runs.len() as u32);
                throughput.insert(label.clone(), figure(&per_run, timed_out));
                per_label_runs.insert(label.clone(), per_run);
            }
            let mut sqlite_by_version = Vec::new();
            for label in &manifest.labels {
                // The SQLite measured in this version's own runs.
                let mut beside = BTreeMap::new();
                for run in 1..=manifest.runs {
                    if let Some(values) = samples.get(&key_of(&format!("sqlite@{label}"), run)) {
                        let mut values = values.clone();
                        if let Some(m) = median(&mut values) {
                            beside.insert(run, m);
                        }
                    }
                }
                let mut ratios = BTreeMap::new();
                for (run, value) in &per_label_runs[label] {
                    if let Some(sqlite) = beside.get(run) {
                        ratios.insert(*run, value / sqlite);
                    }
                }
                vs_sqlite.insert(label.clone(), figure(&ratios, 0));
                if let Some(m) = figure(&beside, 0).median {
                    sqlite_by_version.push(m);
                }
            }
            // Control group: SQLite beside each version must agree.
            let low = sqlite_by_version.iter().copied().reduce(f64::min);
            let high = sqlite_by_version.iter().copied().reduce(f64::max);
            if let (Some(low), Some(high)) = (low, high)
                && low > 0.0
                && (high - low) / low > SQLITE_DRIFT
                && entry.unit != "open"
            {
                blockers.insert(format!(
                    "{pair} {}: SQLite differed {:.0}% between versions' runs; the host was not steady",
                    entry.id,
                    (high - low) / low * 100.0
                ));
            }
            let mut speedup = BTreeMap::new();
            if let Some((baseline, later)) = manifest.labels.split_first() {
                let base = &per_label_runs[baseline];
                for label in later {
                    let current = &per_label_runs[label];
                    let mut paired = BTreeMap::new();
                    for (run, value) in current {
                        if let Some(old) = base.get(run) {
                            paired.insert(*run, value / old);
                        }
                    }
                    let fig = figure(&paired, 0);
                    let new_fig = &throughput[label];
                    let old_fig = &throughput[baseline];
                    let exceeds_noise = match (new_fig.min, new_fig.max, old_fig.min, old_fig.max) {
                        (Some(new_min), Some(new_max), Some(old_min), Some(old_max)) => {
                            new_min > old_max || new_max < old_min
                        }
                        _ => false,
                    };
                    speedup.insert(
                        format!("{label}/{baseline}"),
                        Speedup {
                            median: fig.median,
                            min: fig.min,
                            max: fig.max,
                            exceeds_noise,
                        },
                    );
                }
            }
            let digest_agrees = digests
                .get(&(pair.clone(), entry.id.to_owned()))
                .is_none_or(|set| set.len() <= 1);
            if !digest_agrees {
                blockers.insert(format!(
                    "{pair} {}: engines or versions disagree on the result",
                    entry.id
                ));
            }
            workloads.push(WorkloadSummary {
                pair: pair.clone(),
                id: entry.id.to_owned(),
                unit: entry.unit.to_owned(),
                summary: entry.summary.to_owned(),
                throughput,
                vs_sqlite,
                speedup,
                digest_agrees,
            });
        }
    }

    Ok(Summary {
        schema: SUMMARY_SCHEMA.to_owned(),
        bundle: manifest.bundle,
        labels: manifest.labels,
        rows: manifest.rows,
        runs: manifest.runs,
        publishable: blockers.is_empty(),
        blockers: blockers.into_iter().collect(),
        sqlite_version,
        raw,
        workloads,
    })
}

/// Write `summary.json`, or with `check` confirm it matches the records.
pub fn write_or_check(bundle: &Path, check: bool) -> Result<Summary> {
    let summary = summarize(bundle)?;
    let path = bundle.join("summary.json");
    let rendered = format!("{}\n", serde_json::to_string_pretty(&summary)?);
    if check {
        let on_disk = fs::read_to_string(&path).context("read summary.json")?;
        if on_disk != rendered {
            bail!(
                "{} does not match its raw records; run `redline-scoreboard summarize --bundle {}`",
                path.display(),
                bundle.display()
            );
        }
    } else {
        fs::write(&path, rendered)?;
    }
    Ok(summary)
}
