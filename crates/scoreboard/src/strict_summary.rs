//! Proof of Strict repetition pairing and its local SQLite control.

use std::collections::BTreeMap;

use crate::pair::Engine;
use crate::strict;
use crate::summary::{BundleManifest, Record, SQLITE, median};

#[derive(Default)]
pub(crate) struct Stats {
    pub blockers: Vec<String>,
    pub vs_sqlite: BTreeMap<String, BTreeMap<u32, f64>>,
    /// max over runs of median over paired repetitions of max(S)/min(S)-1.
    pub control_drift: f64,
}

pub(crate) fn analyze(records: &[Record], manifest: &BundleManifest, workload: &str) -> Stats {
    let mut stats = Stats::default();
    let reps = manifest
        .strict_reps
        .unwrap_or(strict::MIN_REPS)
        .max(strict::MIN_REPS);
    let mut by_pair: BTreeMap<(u32, u32), Vec<&Record>> = BTreeMap::new();
    for record in records
        .iter()
        .filter(|r| r.pair == "strict" && r.workload == workload)
    {
        let Some(rep) = record.rep else {
            stats
                .blockers
                .push(format!("strict {workload}: missing repetition id"));
            continue;
        };
        if record.run == 0 || record.run > manifest.runs || rep >= reps || record.seq.is_none() {
            stats.blockers.push(format!(
                "strict {workload}: invalid run/repetition/sequence identity"
            ));
            continue;
        }
        by_pair.entry((record.run, rep)).or_default().push(record);
    }
    for run in 1..=manifest.runs {
        let mut drifts = Vec::new();
        let mut ratios: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for rep in 0..reps {
            let mut got = by_pair.remove(&(run, rep)).unwrap_or_default();
            let expected = strict::order(manifest.labels.len().clamp(1, 2), run, rep);
            got.sort_by_key(|r| r.seq);
            let ordered = got.len() == expected.len()
                && got
                    .windows(2)
                    .all(|r| r[0].seq.and_then(|s| s.checked_add(1)) == r[1].seq)
                && got.iter().zip(&expected).all(|(record, (index, engine))| {
                    manifest.labels.get(*index).is_some_and(|label| {
                        record.with == *label
                            && record.label
                                == if *engine == Engine::Sqlite {
                                    SQLITE
                                } else {
                                    label
                                }
                    })
                });
            if !ordered {
                stats.blockers.push(format!(
                    "strict {workload} run {run} rep {rep}: missing, duplicate or separated engine/control pair"
                ));
                continue;
            }
            let mut controls = Vec::new();
            for label in &manifest.labels {
                let own = got.iter().find(|r| r.label == *label);
                let control = got.iter().find(|r| r.label == SQLITE && r.with == *label);
                let (Some(own), Some(control)) = (own, control) else {
                    continue;
                };
                if own.status != "ok"
                    || control.status != "ok"
                    || own.elapsed_ns == 0
                    || control.elapsed_ns == 0
                    || own.ops == 0
                    || own.ops != control.ops
                {
                    stats.blockers.push(format!(
                        "strict {workload} run {run} rep {rep}: failed or unequal paired work"
                    ));
                    continue;
                }
                let control_rate = control.ops as f64 / control.elapsed_ns as f64;
                controls.push(control_rate);
                ratios
                    .entry(label.clone())
                    .or_default()
                    .push(control.elapsed_ns as f64 / own.elapsed_ns as f64);
            }
            if controls.len() == manifest.labels.len() {
                let low = controls.iter().copied().reduce(f64::min).unwrap_or(1.0);
                let high = controls.iter().copied().reduce(f64::max).unwrap_or(1.0);
                drifts.push(high / low - 1.0);
            }
        }
        if let Some(drift) = median(&mut drifts) {
            stats.control_drift = stats.control_drift.max(drift);
        }
        for (label, mut values) in ratios {
            if let Some(ratio) = median(&mut values) {
                stats.vs_sqlite.entry(label).or_default().insert(run, ratio);
            }
        }
    }
    stats
}
