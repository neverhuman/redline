//! The bundle's own files as a summary reads them: bundle.json, host.json,
//! the parts of a build contract and a build record it relies on, and the
//! case lists, every path kept inside the bundle.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// How every run of the bundle was measured.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Protocol {
    pub suite: String,
    pub workers: usize,
    pub repetitions: usize,
    pub warmup: usize,
    pub order: String,
    /// The taskset CPU list every run was pinned to; null when unpinned.
    pub cpus: Option<String>,
    pub tmp_root: String,
    /// `normal`: every run set REDLINEDB_DEFAULT_DURABILITY=normal (and
    /// REDLINEDB_QUIET_DURABILITY=1); `default`: neither was set, so each
    /// binary ran at its built-in default durability.
    pub durability: String,
    pub measurement_boundary: String,
    pub max_loadavg: f64,
    pub command: String,
}

/// A file inside the bundle, by its path relative to the bundle.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileRef {
    pub path: String,
    pub sha256: String,
    /// Where the bundle's copy came from.
    pub source: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BundleManifest {
    pub(super) schema_version: String,
    pub(super) bundle: String,
    pub(super) started_at_utc: String,
    pub(super) finished_at_utc: String,
    pub(super) runs_per_label: usize,
    pub(super) protocol: Protocol,
    pub(super) cases: CaseSet,
    pub(super) medium_cohort: Option<FileRef>,
    pub(super) labels: Vec<LabelEntry>,
    pub(super) runs: Vec<RunEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CaseSet {
    pub(super) manifest: String,
    pub(super) count: usize,
    pub(super) case_list: Option<FileRef>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LabelEntry {
    pub(super) label: String,
    pub(super) binary: String,
    pub(super) binary_sha256: String,
    pub(super) source_ref: Option<String>,
    pub(super) source_commit: Option<String>,
    pub(super) build_contract: String,
    pub(super) build_record: Option<String>,
    /// Whether the binary has the REDLINEDB_DEFAULT_DURABILITY knob (v4.0.9
    /// and older do not, and always run their built-in Strict default).
    pub(super) durability_env: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RunEntry {
    pub(super) sequence: usize,
    pub(super) label: String,
    pub(super) run: usize,
    pub(super) raw: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HostRecord {
    pub(super) schema_version: String,
    pub(super) hostname: String,
    pub(super) kernel: String,
    pub(super) cpu_model: String,
    pub(super) nproc: usize,
    pub(super) pinned_cpus: Option<String>,
    /// `stat -f -c %T` of the directory the runs' temp roots were made in.
    pub(super) tmp_filesystem: String,
    pub(super) governors: BTreeMap<String, String>,
    pub(super) runner_units_active: Vec<String>,
    pub(super) max_loadavg: f64,
    pub(super) runs: Vec<HostRun>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HostRun {
    pub(super) sequence: usize,
    pub(super) label: String,
    pub(super) run: usize,
    pub(super) started_at_utc: String,
    pub(super) finished_at_utc: String,
    pub(super) loadavg_before: [f64; 3],
    pub(super) loadavg_after: [f64; 3],
    pub(super) runner_jobs_before: usize,
    pub(super) runner_jobs_after: usize,
    pub(super) runner_exit: i32,
    pub(super) accepted: bool,
    pub(super) reason: Option<String>,
}

/// The parts of build-contract.json (BM3-05) a bundle relies on.
#[derive(Debug, Deserialize)]
pub(super) struct ContractView {
    pub(super) schema_version: String,
    pub(super) target: TargetView,
    pub(super) reference: Identity,
    pub(super) runner: Identity,
    pub(super) optimization: OptimizationView,
}

#[derive(Debug, Deserialize)]
pub(super) struct TargetView {
    pub(super) sha256: String,
    pub(super) version: String,
    pub(super) build: DeclaredBuildView,
}

#[derive(Debug, Deserialize)]
pub(super) struct OptimizationView {
    pub(super) pgo_training_corpus: Option<String>,
}

/// A binary as a build contract names it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Identity {
    pub path: String,
    pub sha256: String,
    pub version: String,
}

/// A target's build as its builder declared it in the contract.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DeclaredBuildView {
    pub declared: bool,
    pub profile: Option<String>,
    pub features: Option<String>,
    pub rustflags: Option<String>,
}

/// The parts of scripts/perf/build-version.sh's build.json a bundle uses.
#[derive(Debug, Deserialize)]
pub(super) struct VersionBuild {
    pub(super) schema_version: String,
    pub(super) source_commit: String,
    pub(super) binary_sha256: String,
    pub(super) rustc_verbose_version: Vec<String>,
    pub(super) profile: String,
    pub(super) rustflags: String,
    pub(super) pgo: bool,
}

/// A case list's ids (five digits, `#` comments allowed), after checking
/// the bundle's copy against its recorded SHA-256.
pub(super) fn read_case_list(dir: &Path, file: &FileRef) -> Result<BTreeSet<String>> {
    let path = bundle_path(dir, &file.path)?;
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if digest != file.sha256 {
        bail!(
            "{} has SHA-256 {digest}, but the bundle records {}",
            file.path,
            file.sha256
        );
    }
    let text = String::from_utf8(bytes).with_context(|| format!("{} is not UTF-8", file.path))?;
    parse_case_list(&text).with_context(|| format!("case list {}", file.path))
}

/// Parses a case list: one numeric case id per line, `#` starting a
/// comment. Ids are returned as the five-digit display ids raw records
/// carry; a repeated id or an empty list is an error.
pub fn parse_case_list(text: &str) -> Result<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        let id = line.split('#').next().unwrap_or_default().trim();
        if id.is_empty() {
            continue;
        }
        if id.len() > 5 || !id.bytes().all(|byte| byte.is_ascii_digit()) {
            bail!("line {}: {id:?} is not a case id", index + 1);
        }
        let id = format!("{:0>5}", id);
        if !ids.insert(id.clone()) {
            bail!("line {}: case {id} is listed twice", index + 1);
        }
    }
    if ids.is_empty() {
        bail!("lists no case");
    }
    Ok(ids)
}

/// A path inside the bundle: relative, with no `..`, `.` or root.
pub(super) fn bundle_path(dir: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    if relative.is_empty()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        bail!("{relative:?} is not a plain path inside the bundle");
    }
    Ok(dir.join(path))
}

pub(super) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}
