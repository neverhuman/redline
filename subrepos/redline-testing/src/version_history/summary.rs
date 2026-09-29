//! The shape of a release bench bundle's `summary.json`, as
//! `perf_evidence summarize-bundle` writes it.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct Summary {
    pub(super) schema_version: String,
    pub(super) bundle: String,
    pub(super) finished_at_utc: String,
    pub(super) protocol: Protocol,
    pub(super) host: Host,
    pub(super) runner: Identity,
    pub(super) reference: Identity,
    pub(super) corpus: Corpus,
    pub(super) runs_per_label: usize,
    pub(super) common_pass_set: CommonPassSet,
    pub(super) labels: Vec<Label>,
    pub(super) publishable: bool,
    pub(super) publication_blockers: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Protocol {
    pub(super) workers: usize,
    pub(super) repetitions: usize,
    pub(super) warmup: usize,
    pub(super) order: String,
    pub(super) cpus: Option<String>,
    pub(super) durability: String,
    pub(super) measurement_boundary: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct Host {
    pub(super) cpu_model: String,
    pub(super) nproc: usize,
    pub(super) kernel: String,
    pub(super) tmp_filesystem: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct Identity {
    pub(super) sha256: String,
    pub(super) version: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct Corpus {
    pub(super) cases: usize,
    pub(super) narrowed_by: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CommonPassSet {
    pub(super) cases: usize,
}

#[derive(Debug, Deserialize)]
pub(super) struct Label {
    pub(super) label: String,
    pub(super) source_commit: Option<String>,
    pub(super) build: Build,
    pub(super) passed_cases: usize,
    pub(super) flaky_cases: usize,
    pub(super) runs: Vec<Run>,
    pub(super) common: Stats,
}

#[derive(Debug, Deserialize)]
pub(super) struct Build {
    pub(super) declared: bool,
    pub(super) profile: Option<String>,
    pub(super) rustflags: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Run {
    pub(super) raw: String,
    pub(super) raw_sha256: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct Stats {
    pub(super) median: Spread,
    pub(super) p95: Spread,
    pub(super) delta_vs_previous: Option<Delta>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Spread {
    pub(super) value: f64,
    pub(super) min: f64,
    pub(super) max: f64,
    pub(super) per_run: Vec<f64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Delta {
    pub(super) median_change_pct: f64,
    pub(super) verdict: Verdict,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Verdict {
    WithinNoise,
    ExceedsNoise,
    Unassessed,
}
