use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use super::types::{RawRecord, RenderedReport, Score, SvgArtifact};

pub(crate) fn verify_existing(
    input: &Path,
    raw_out: &Path,
    summary_out: &Path,
    ranked_out: &Path,
    manifest_out: &Path,
    report_provenance_out: &Path,
    readme_out: &Path,
    rendered: &RenderedReport,
    svg_artifacts: &[SvgArtifact],
) -> Result<()> {
    verify_text(input, &rendered.raw)?;
    verify_text(raw_out, &rendered.raw)?;
    verify_text(summary_out, &rendered.summary)?;
    verify_text(ranked_out, &rendered.ranked)?;
    verify_text(manifest_out, &rendered.manifest)?;
    verify_text(report_provenance_out, &rendered.report_provenance)?;
    verify_text(readme_out, &rendered.readme)?;
    for artifact in svg_artifacts {
        verify_text(&artifact.path, &artifact.contents)?;
    }
    Ok(())
}

pub(crate) fn verify_text(path: &Path, expected: &str) -> Result<()> {
    let actual = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    if actual != expected {
        bail!("artifact drift detected: {}", path.display());
    }
    Ok(())
}

pub(crate) fn write_text(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

pub(crate) fn sha256_hex(text: &str) -> String {
    sha256_bytes(text.as_bytes())
}

pub(crate) fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn csv(value: &str) -> String {
    if value.contains([',', '"', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

pub(crate) fn is_measured(record: &RawRecord) -> bool {
    record.status == "passed"
        && (record.repetition_index.is_some() || record.sample_role.starts_with("measured"))
}

pub(crate) fn median(values: impl Iterator<Item = u128>) -> u128 {
    let mut values = values.collect::<Vec<_>>();
    values.sort_unstable();
    values[values.len() / 2]
}

pub(crate) fn median_u64(values: impl Iterator<Item = u64>) -> Option<u64> {
    let mut values = values.collect::<Vec<_>>();
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(values[values.len() / 2])
}

pub(crate) fn parse_score(path: &Path) -> Result<Score> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    Ok(Score {
        score: value["score"].as_u64().unwrap_or(0),
        status: value["status"].as_str().unwrap_or("unknown").to_owned(),
    })
}

/// This invocation's arguments without `--check`, with the program named by
/// its file name: where the renderer is installed is not report output.
pub(crate) fn normalized_command_line() -> Vec<String> {
    normalize_command_line(std::env::args())
}

pub(crate) fn normalize_command_line(args: impl IntoIterator<Item = String>) -> Vec<String> {
    args.into_iter()
        .enumerate()
        .filter(|(_, arg)| arg != "--check")
        .map(|(index, arg)| {
            if index == 0 {
                Path::new(&arg)
                    .file_name()
                    .map_or(arg.clone(), |name| name.to_string_lossy().into_owned())
            } else {
                arg
            }
        })
        .collect()
}
