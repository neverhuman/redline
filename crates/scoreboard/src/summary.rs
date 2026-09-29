//! Summarize a bundle: the raw records of several versions, each measured
//! beside SQLite in the same runs, into medians, paired speedups and
//! publication blockers. `summary.json` is derived; the raw records are
//! the evidence, and `--check` recomputes the summary from them.
//!
//! A bundle is publishable only when it is complete and clean: every
//! version and SQLite beside it measured every workload of every pair in
//! every run; nothing failed or timed out; every record has the manifest's
//! scale and full work; each series ran one engine version; every result
//! agreed; no CI job ran during a run; the normal pair ran on tmpfs; and
//! SQLite, the control group, measured alike beside every version.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::pair::Pair;
use crate::run::selected;

pub const SUMMARY_SCHEMA: &str = "redline-scoreboard-summary-v1";
pub const BUNDLE_SCHEMA: &str = "redline-scoreboard-bundle-v1";

/// Fewest runs of each version a publishable bundle holds.
pub const MIN_RUNS: u32 = 3;
/// SQLite is the control group: its medians beside different versions on
/// the same host must agree this closely, or the host drifted.
pub const SQLITE_DRIFT: f64 = 0.10;
/// The series name of SQLite records; no version may use it.
pub const SQLITE: &str = "sqlite";

/// `bundle.json`, written by `scripts/perf/scoreboard-bench.sh`.
#[derive(Debug, Deserialize)]
pub struct BundleManifest {
    pub schema: String,
    pub bundle: String,
    /// Versions in order; the first is the baseline speedups divide by.
    pub labels: Vec<String>,
    pub rows: u64,
    pub runs: u32,
    #[serde(default = "default_reps")]
    pub reps: u32,
    pub pairs: Vec<Pair>,
}

fn default_reps() -> u32 {
    5
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Summary {
    pub schema: String,
    pub bundle: String,
    pub labels: Vec<String>,
    pub rows: u64,
    pub runs: u32,
    pub reps: u32,
    pub publishable: bool,
    pub blockers: Vec<String>,
    pub sqlite_version: Option<String>,
    /// The file system the normal pair ran on, from host.json.
    pub normal_dir_fs: Option<String>,
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
    /// Throughput of each version, and of SQLite beside each version under
    /// `sqlite@<version>`.
    pub throughput: BTreeMap<String, Figure>,
    /// Each version's throughput over SQLite's beside it, run by run.
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
    #[serde(default)]
    with: String,
    run: u32,
    pair: String,
    workload: String,
    #[serde(default)]
    rows: u64,
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
}

#[derive(Debug, Deserialize)]
struct HostRun {
    label: String,
    run: u32,
    pair: String,
    runner_jobs_before: u64,
    runner_jobs_after: u64,
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

fn figure(per_run: &BTreeMap<u32, f64>) -> Figure {
    let mut values: Vec<f64> = per_run.values().copied().collect();
    Figure {
        min: values.iter().copied().reduce(f64::min),
        max: values.iter().copied().reduce(f64::max),
        median: median(&mut values),
    }
}

fn sqlite_series(label: &str) -> String {
    format!("{SQLITE}@{label}")
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
    let mut blockers = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for label in &manifest.labels {
        if label == SQLITE || !seen.insert(label) {
            blockers.insert(format!("version label {label} is reserved or repeated"));
        }
    }
    if manifest.runs < MIN_RUNS {
        blockers.insert(format!("{} runs, fewer than {MIN_RUNS}", manifest.runs));
    }

    let host: Option<serde_json::Value> = fs::read(bundle.join("host.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let normal_dir_fs = host
        .as_ref()
        .and_then(|host| host["normal_dir_fs"].as_str().map(str::to_owned));
    if host.is_none() {
        blockers.insert("no host.json".to_owned());
    } else if manifest.pairs.contains(&Pair::Normal) && normal_dir_fs.as_deref() != Some("tmpfs") {
        blockers.insert(format!(
            "the normal pair ran on {}, not tmpfs",
            normal_dir_fs.as_deref().unwrap_or("an unknown file system")
        ));
    }
    match fs::read_to_string(bundle.join("runs.jsonl")) {
        Ok(text) => {
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                let run: HostRun = serde_json::from_str(line).context("parse runs.jsonl")?;
                if run.runner_jobs_before > 0 || run.runner_jobs_after > 0 {
                    blockers.insert(format!(
                        "{} run {} {}: a CI job ran on the host during the run",
                        run.label, run.run, run.pair
                    ));
                }
            }
        }
        Err(_) => {
            blockers.insert("no runs.jsonl host record".to_owned());
        }
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

    // (pair, workload, series, run) -> throughput of each repetition
    let mut samples: BTreeMap<(String, String, String, u32), Vec<f64>> = BTreeMap::new();
    let mut digests: BTreeMap<(String, String), BTreeSet<i64>> = BTreeMap::new();
    let mut versions: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for record in &records {
        let series = if record.label == SQLITE {
            sqlite_series(&record.with)
        } else {
            record.label.clone()
        };
        if record.status != "ok" {
            blockers.insert(format!(
                "{} {} {} run {}: {}",
                series, record.pair, record.workload, record.run, record.status
            ));
            continue;
        }
        if record.rows != manifest.rows {
            blockers.insert(format!(
                "{series} {}: measured at {} rows, the bundle says {}",
                record.workload, record.rows, manifest.rows
            ));
        }
        if record.work_divisor != 1 {
            blockers.insert(format!(
                "{series} {}: measured with work divisor {}",
                record.workload, record.work_divisor
            ));
        }
        let key = if record.label == SQLITE {
            SQLITE.to_owned()
        } else {
            record.label.clone()
        };
        versions
            .entry(key)
            .or_default()
            .insert(record.engine_version.clone());
        if record.elapsed_ns == 0 || record.ops == 0 {
            blockers.insert(format!("{series} {}: recorded no work", record.workload));
            continue;
        }
        let throughput = record.ops as f64 * 1e9 / record.elapsed_ns as f64;
        samples
            .entry((
                record.pair.clone(),
                record.workload.clone(),
                series,
                record.run,
            ))
            .or_default()
            .push(throughput);
        digests
            .entry((record.pair.clone(), record.workload.clone()))
            .or_default()
            .insert(record.digest);
    }
    for (series, names) in &versions {
        if names.len() > 1 {
            blockers.insert(format!("{series} ran several engine versions: {names:?}"));
        }
    }
    let sqlite_version = versions
        .get(SQLITE)
        .and_then(|names| names.iter().next().cloned());

    let mut workloads = Vec::new();
    for pair in &manifest.pairs {
        let pair_name = pair.to_string();
        for entry in selected(&None, *pair)? {
            let per_run = |series: &str| -> BTreeMap<u32, f64> {
                let mut out = BTreeMap::new();
                for run in 1..=manifest.runs {
                    let key = (
                        pair_name.clone(),
                        entry.id.to_owned(),
                        series.to_owned(),
                        run,
                    );
                    if let Some(values) = samples.get(&key) {
                        let mut values = values.clone();
                        if let Some(m) = median(&mut values) {
                            out.insert(run, m);
                        }
                    }
                }
                out
            };
            let mut throughput = BTreeMap::new();
            let mut vs_sqlite = BTreeMap::new();
            let mut runs_of = BTreeMap::new();
            let mut sqlite_medians = Vec::new();
            for label in &manifest.labels {
                let own = per_run(label);
                let beside = per_run(&sqlite_series(label));
                for (series, got) in [(label.clone(), &own), (sqlite_series(label), &beside)] {
                    if got.len() < manifest.runs as usize {
                        blockers.insert(format!(
                            "{series} {pair_name} {}: {} of {} runs measured",
                            entry.id,
                            got.len(),
                            manifest.runs
                        ));
                    }
                }
                let mut ratios = BTreeMap::new();
                for (run, value) in &own {
                    if let Some(sqlite) = beside.get(run) {
                        ratios.insert(*run, value / sqlite);
                    }
                }
                vs_sqlite.insert(label.clone(), figure(&ratios));
                let beside_figure = figure(&beside);
                if let Some(m) = beside_figure.median {
                    sqlite_medians.push(m);
                }
                throughput.insert(label.clone(), figure(&own));
                throughput.insert(sqlite_series(label), beside_figure);
                runs_of.insert(label.clone(), own);
            }
            // Open times are a few milliseconds and vary more than the
            // work the other workloads time, so they are not held to it.
            let low = sqlite_medians.iter().copied().reduce(f64::min);
            let high = sqlite_medians.iter().copied().reduce(f64::max);
            if let (Some(low), Some(high)) = (low, high)
                && low > 0.0
                && (high - low) / low > SQLITE_DRIFT
                && entry.unit != "open"
            {
                blockers.insert(format!(
                    "{pair_name} {}: SQLite differed {:.0}% between versions' runs; the host was not steady",
                    entry.id,
                    (high - low) / low * 100.0
                ));
            }
            let mut speedup = BTreeMap::new();
            if let Some((baseline, later)) = manifest.labels.split_first() {
                let base = &runs_of[baseline];
                for label in later {
                    let mut paired = BTreeMap::new();
                    for (run, value) in &runs_of[label] {
                        if let Some(old) = base.get(run) {
                            paired.insert(*run, value / old);
                        }
                    }
                    let fig = figure(&paired);
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
                .get(&(pair_name.clone(), entry.id.to_owned()))
                .is_none_or(|set| set.len() <= 1);
            if !digest_agrees {
                blockers.insert(format!(
                    "{pair_name} {}: engines or versions disagree on the result",
                    entry.id
                ));
            }
            workloads.push(WorkloadSummary {
                pair: pair_name.clone(),
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
        reps: manifest.reps,
        publishable: blockers.is_empty(),
        blockers: blockers.into_iter().collect(),
        sqlite_version,
        normal_dir_fs,
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
