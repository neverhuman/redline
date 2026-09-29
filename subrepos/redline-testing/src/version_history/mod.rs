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

use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

mod render;
mod summary;

use render::render_block;
use summary::Summary;

pub(crate) const BEGIN: &str = "<!-- version-history:begin -->";
pub(crate) const END: &str = "<!-- version-history:end -->";
pub(crate) const SUMMARY_SCHEMA: &str = "redline-release-bench-summary-v1";

pub(crate) struct VersionHistoryOptions {
    pub(crate) bundle: PathBuf,
    pub(crate) readme: Option<PathBuf>,
    pub(crate) check: bool,
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
