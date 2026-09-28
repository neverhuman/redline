//! `version-history`: the README "Versions over time" table (L-05),
//! rendered from a release bench bundle's `summary.json`.
//!
//! The bundle is measured by RedlineDB's `scripts/perf/release-bench.sh`
//! and summarized by `perf_evidence summarize-bundle`, which re-derives
//! every number from the bundle's raw records and lists what keeps the
//! bundle from being publishable. This renderer adds no statistics. It
//! refuses a summary whose raw files changed after it was written, and it
//! writes a README block only from a publishable bundle: the whole corpus,
//! at least 3 runs per version, one case at a time on pinned CPUs and
//! tmpfs, and declared, identical release builds without PGO. Without
//! `--readme` it prints the block, marked not publishable when it is not.
//!
//! The block sits between `<!-- version-history:begin -->` and
//! `<!-- version-history:end -->`; both markers must already be in the
//! README exactly once, and nothing outside them is touched.

use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub(crate) const BEGIN: &str = "<!-- version-history:begin -->";
pub(crate) const END: &str = "<!-- version-history:end -->";
pub(crate) const SUMMARY_SCHEMA: &str = "redline-release-bench-summary-v1";

pub(crate) struct VersionHistoryOptions {
    pub(crate) bundle: PathBuf,
    pub(crate) readme: Option<PathBuf>,
    pub(crate) check: bool,
}

#[derive(Debug, Deserialize)]
struct Summary {
    schema_version: String,
    bundle: String,
    finished_at_utc: String,
    protocol: Protocol,
    host: Host,
    runner: Identity,
    reference: Identity,
    corpus: Corpus,
    runs_per_label: usize,
    common_pass_set: CommonPassSet,
    labels: Vec<Label>,
    publishable: bool,
    publication_blockers: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Protocol {
    workers: usize,
    repetitions: usize,
    warmup: usize,
    order: String,
    cpus: Option<String>,
    durability: String,
    measurement_boundary: String,
}

#[derive(Debug, Deserialize)]
struct Host {
    cpu_model: String,
    nproc: usize,
    kernel: String,
    tmp_filesystem: String,
}

#[derive(Debug, Deserialize)]
struct Identity {
    sha256: String,
    version: String,
}

#[derive(Debug, Deserialize)]
struct Corpus {
    cases: usize,
    narrowed_by: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct CommonPassSet {
    cases: usize,
}

#[derive(Debug, Deserialize)]
struct Label {
    label: String,
    source_commit: Option<String>,
    build: Build,
    passed_cases: usize,
    flaky_cases: usize,
    runs: Vec<Run>,
    common: Stats,
}

#[derive(Debug, Deserialize)]
struct Build {
    declared: bool,
    profile: Option<String>,
    rustflags: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Run {
    raw: String,
    raw_sha256: String,
}

#[derive(Debug, Deserialize)]
struct Stats {
    median: Spread,
    p95: Spread,
    delta_vs_previous: Option<Delta>,
}

#[derive(Debug, Deserialize)]
struct Spread {
    value: f64,
    min: f64,
    max: f64,
    per_run: Vec<f64>,
}

#[derive(Debug, Deserialize)]
struct Delta {
    median_change_pct: f64,
    verdict: Verdict,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Verdict {
    WithinNoise,
    ExceedsNoise,
    Unassessed,
}

pub(crate) fn run(options: VersionHistoryOptions) -> Result<()> {
    if options.check && options.readme.is_none() {
        bail!("--check compares a README block; give --readme");
    }
    let summary = load_summary(&options.bundle)?;
    let Some(readme) = &options.readme else {
        print!(
            "{}",
            render_block(&summary, &options.bundle.display().to_string())?
        );
        return Ok(());
    };
    if !summary.publishable {
        bail!(
            "bundle {} is not publishable, so it cannot write the README version table: {}",
            summary.bundle,
            summary.publication_blockers.join("; ")
        );
    }
    let link = bundle_link(readme, &options.bundle)?;
    let text = fs::read_to_string(readme).with_context(|| format!("read {}", readme.display()))?;
    let updated = replace_block(&text, &render_block(&summary, &link)?)
        .with_context(|| format!("README {}", readme.display()))?;
    if options.check {
        if updated != text {
            bail!(
                "the version-history block in {} is not what bundle {} renders; rerun redline-testing version-history --bundle {} --readme {}",
                readme.display(),
                summary.bundle,
                options.bundle.display(),
                readme.display()
            );
        }
    } else if updated != text {
        fs::write(readme, updated).with_context(|| format!("write {}", readme.display()))?;
    }
    Ok(())
}

/// The bundle's summary.json, after checking its schema, its shape and
/// that every raw file it summarizes is still the file it summarized.
fn load_summary(bundle: &Path) -> Result<Summary> {
    let path = bundle.join("summary.json");
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let summary: Summary =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    if summary.schema_version != SUMMARY_SCHEMA {
        bail!(
            "{} has schema {:?}, not {SUMMARY_SCHEMA}",
            path.display(),
            summary.schema_version
        );
    }
    if summary.labels.is_empty() {
        bail!("{} lists no version", path.display());
    }
    for label in &summary.labels {
        if label.label.is_empty()
            || !label
                .label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
        {
            bail!("version label {:?} is not a plain name", label.label);
        }
        if let Some(commit) = &label.source_commit
            && (commit.is_empty() || !commit.chars().all(|c| c.is_ascii_hexdigit()))
        {
            bail!("{}: source commit {commit:?} is not a hex id", label.label);
        }
        if label.runs.len() != summary.runs_per_label
            || label.common.median.per_run.len() != summary.runs_per_label
            || label.common.p95.per_run.len() != summary.runs_per_label
        {
            bail!(
                "{}: the summary does not hold {} runs",
                label.label,
                summary.runs_per_label
            );
        }
        for run in &label.runs {
            let raw = inside(bundle, &run.raw)?;
            let bytes = fs::read(&raw).with_context(|| format!("read {}", raw.display()))?;
            let digest = format!("{:x}", Sha256::digest(&bytes));
            if digest != run.raw_sha256 {
                bail!(
                    "{} has SHA-256 {digest}, not the {} the summary was computed from; rerun perf_evidence summarize-bundle",
                    raw.display(),
                    run.raw_sha256
                );
            }
        }
    }
    Ok(summary)
}

/// The block between the markers: the table and its footnote.
fn render_block(summary: &Summary, link: &str) -> Result<String> {
    let mut out = String::new();
    writeln!(
        out,
        "<!-- Generated by `redline-testing version-history` from {link}/summary.json; do not edit by hand. -->"
    )?;
    if !summary.publishable {
        writeln!(
            out,
            "\n> **Not publishable:** {}.",
            summary.publication_blockers.join("; ")
        )?;
    }
    writeln!(out)?;
    let of = if summary.corpus.narrowed_by.is_some() {
        format!("of {} listed cases", summary.corpus.cases)
    } else {
        format!("of {}, today's corpus", summary.corpus.cases)
    };
    writeln!(
        out,
        "| Version | Commit | SQLite corpus passed ({of}) | Median latency ratio vs SQLite (lower is better) | p95 | Δ median vs previous |"
    )?;
    writeln!(out, "| --- | --- | ---: | ---: | ---: | --- |")?;
    for label in &summary.labels {
        let commit = label.source_commit.as_deref().map_or_else(
            || "not recorded".to_owned(),
            |commit| format!("`{}`", &commit[..commit.len().min(9)]),
        );
        let passed = if label.flaky_cases > 0 {
            format!("{} (+{} flaky)", label.passed_cases, label.flaky_cases)
        } else {
            label.passed_cases.to_string()
        };
        writeln!(
            out,
            "| {} | {commit} | {passed} | {} | {} | {} |",
            label.label,
            spread(&label.common.median),
            spread(&label.common.p95),
            delta(
                label.common.delta_vs_previous.as_ref(),
                summary.runs_per_label
            )
        )?;
    }
    writeln!(out)?;
    let runs = summary.runs_per_label;
    writeln!(
        out,
        "Each ratio compares RedlineDB with SQLite on the same case, and lower is better. The time is \
         per-case CLI process wall time (`{boundary}`): every sample starts a fresh `redlinedb` or \
         `sqlite3` shell, so process start-up is included. A case's ratio is RedlineDB's median over \
         SQLite's median across {reps} measured repetitions after {warmup} warmup. The table shows the \
         median and nearest-rank p95 of those ratios over the {common} cases every version passed in \
         every run (the common pass set). Each version ran {runs} time(s), interleaved with the others. \
         The figure is the median run, and the parentheses give the min–max across runs. A change is \
         shown only when the two versions' ranges do not overlap; otherwise it reads \"within noise\". \
         \"Passed\" counts the corpus cases a version passed in every run.",
        boundary = summary.protocol.measurement_boundary,
        reps = summary.protocol.repetitions,
        warmup = summary.protocol.warmup,
        common = summary.common_pass_set.cases,
    )?;
    writeln!(out)?;
    let pinned = summary.protocol.cpus.as_deref().map_or_else(
        || "unpinned".to_owned(),
        |cpus| format!("pinned to CPUs {cpus}"),
    );
    let durability = if summary.protocol.durability == "normal" {
        "`REDLINEDB_DEFAULT_DURABILITY=normal`".to_owned()
    } else {
        "each version's built-in default durability".to_owned()
    };
    let narrowed = if summary.corpus.narrowed_by.is_some() {
        " The corpus was narrowed by a case list."
    } else {
        ""
    };
    writeln!(
        out,
        "Measured {date} on {cpu} ({nproc} CPUs, {kernel}), {pinned}, {workers} worker(s), \
         `--order {order}`, temp roots on {fs}, {durability}. SQLite reference {reference} \
         (`{reference_sha}`); runner {runner}. {builds}{narrowed} Bundle: [`{link}`]({link}/).",
        date = summary
            .finished_at_utc
            .get(..10)
            .unwrap_or(&summary.finished_at_utc),
        cpu = summary.host.cpu_model,
        nproc = summary.host.nproc,
        kernel = summary.host.kernel,
        workers = summary.protocol.workers,
        order = summary.protocol.order,
        fs = summary.host.tmp_filesystem,
        reference = summary
            .reference
            .version
            .split_whitespace()
            .next()
            .unwrap_or("unknown"),
        reference_sha = &summary.reference.sha256[..summary.reference.sha256.len().min(12)],
        runner = summary.runner.version,
        builds = builds(summary),
    )?;
    Ok(out)
}

fn spread(spread: &Spread) -> String {
    if spread.per_run.len() < 2 {
        format!("{:.3}×", spread.value)
    } else {
        format!("{:.3}× ({:.3}–{:.3})", spread.value, spread.min, spread.max)
    }
}

fn delta(delta: Option<&Delta>, runs: usize) -> String {
    match delta {
        None => "—".to_owned(),
        Some(delta) => match delta.verdict {
            Verdict::WithinNoise => "within noise".to_owned(),
            Verdict::ExceedsNoise => format!("{:+.1}%", delta.median_change_pct),
            Verdict::Unassessed => format!("not assessed ({runs} run)"),
        },
    }
}

fn builds(summary: &Summary) -> String {
    let first = &summary.labels[0].build;
    let alike = summary.labels.iter().all(|label| {
        label.build.declared
            && label.build.profile.as_deref() == Some("release")
            && label.build.rustflags == first.rustflags
    });
    match (&first.rustflags, alike) {
        (Some(flags), true) => format!(
            "Every version was built with the release profile and RUSTFLAGS `{}`, without PGO.",
            if flags.is_empty() { "\"\"" } else { flags }
        ),
        _ => "The versions' builds differ or are undeclared; see the bundle.".to_owned(),
    }
}

/// `text` with the version-history block replaced. Both markers must be
/// present exactly once, in order.
fn replace_block(text: &str, block: &str) -> Result<String> {
    let begins = text.matches(BEGIN).count();
    let ends = text.matches(END).count();
    if begins != 1 || ends != 1 {
        bail!(
            "needs exactly one {BEGIN} and one {END} (found {begins} and {ends}); add them where the table belongs"
        );
    }
    let start = text.find(BEGIN).unwrap_or_default() + BEGIN.len();
    let stop = text.find(END).unwrap_or_default();
    if stop < start {
        bail!("{END} comes before {BEGIN}");
    }
    Ok(format!("{}\n{block}{}", &text[..start], &text[stop..]))
}

/// The bundle as a link relative to the README's directory; the bundle
/// must be inside it.
fn bundle_link(readme: &Path, bundle: &Path) -> Result<String> {
    let readme_dir = readme
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let base = fs::canonicalize(readme_dir)
        .with_context(|| format!("resolve {}", readme_dir.display()))?;
    let target =
        fs::canonicalize(bundle).with_context(|| format!("resolve {}", bundle.display()))?;
    let relative = target.strip_prefix(&base).with_context(|| {
        format!(
            "bundle {} is not inside {}, so the README cannot link to it",
            target.display(),
            base.display()
        )
    })?;
    Ok(relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/"))
}

/// A path inside the bundle: relative, with no `..`, `.` or root.
fn inside(bundle: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    if relative.is_empty()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        bail!("{relative:?} is not a plain path inside the bundle");
    }
    Ok(bundle.join(path))
}

#[cfg(test)]
mod tests;
