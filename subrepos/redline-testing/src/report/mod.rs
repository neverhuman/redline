mod compare;
mod evidence;
mod provenance;
mod qualification;
mod ratio;
mod render;
mod sqlite_check;
mod svg;
mod types;
mod utils;
mod verdicts;

#[cfg(test)]
mod latency_tests;
#[cfg(test)]
mod provenance_tests;
#[cfg(test)]
mod qualification_tests;
#[cfg(test)]
mod test_fixtures;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod verdict_tests;

pub use compare::{jankurai_compare, sentinel};
pub use sqlite_check::{CheckSqliteOptions, check_sqlite};
pub use types::{JankuraiCompareOptions, ReportOptions, SentinelOptions};
// The runner writes its ranked.csv with the same parser, ranking and writer.
pub(crate) use render::{parse_raw_records, rank_cases, ranked_csv};
pub(crate) use utils::is_measured;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use evidence::{
    artifact_names_for_suite, read_official_evidence_versions, validate_completion,
    validate_official_evidence_binding,
};
use provenance::{
    REPORT_PROVENANCE_SCHEMA, Renderer, ReportMode, ReportProvenanceJson, load_report_identity,
};
use qualification::build_sqlite_qualification;
use render::{
    block_text, remove_block_if_present, render_report_block, render_sqlite_badge, replace_block,
    replace_block_if_present,
};
use svg::build_svg_artifacts;
use types::{ManifestJson, RawRecord, RenderedReport, SummaryJson};
use utils::{normalized_command_line, sha256_file, sha256_hex, verify_existing, write_text};

const REPORT_BEGIN: &str = "<!-- sqlite-parity-report:begin -->";
const REPORT_END: &str = "<!-- sqlite-parity-report:end -->";
const BADGE_BEGIN: &str = "<!-- sqlite-parity-badge:begin -->";
const BADGE_END: &str = "<!-- sqlite-parity-badge:end -->";
/// Blocks earlier renderers wrote and this one deletes: placeholder metric
/// cards (KSLOC 1, a "Jankurai score" card of case counts, code shape,
/// Jankurai comparison) and a raw Jankurai comparison JSON dump.
const RETIRED_BLOCKS: [(&str, &str); 2] = [
    (
        "<!-- sqlite-parity-metrics:begin -->",
        "<!-- sqlite-parity-metrics:end -->",
    ),
    (
        "<!-- sqlite-jankurai-breakdown:begin -->",
        "<!-- sqlite-jankurai-breakdown:end -->",
    ),
];

pub fn generate(options: ReportOptions) -> Result<()> {
    let raw_text = fs::read_to_string(&options.input)
        .with_context(|| format!("read raw input {}", options.input.display()))?;
    if let Some(official_evidence) = &options.official_evidence {
        validate_official_evidence_binding(official_evidence, &options.suite, &raw_text)?;
    } else if !options.local_diagnostics || options.check {
        bail!(
            "reporting committed artifacts requires --official-evidence; \
             use --local-diagnostics only for uncommitted local diagnostics"
        );
    }
    let raw_records = parse_raw_records(&raw_text)?;
    if raw_records.is_empty() {
        bail!("sqlite parity report input is empty");
    }
    // Measured identities come from the run alone; see provenance.rs.
    let identity = load_report_identity(&options, &raw_text, &raw_records)?;
    // A run with run provenance must have finished; a historical run
    // predates completion markers.
    if identity.mode == ReportMode::Official
        && let Some(official_evidence) = &options.official_evidence
    {
        validate_completion(
            official_evidence,
            &options.suite,
            &raw_text,
            raw_records.len(),
        )?;
    }

    // One verdict per case, from complete and unique samples, over exactly
    // the manifest's cases (SQ-04).
    let (warmup, repetitions) = sample_plan(&options, &raw_records)?;
    let verdicts = if options.suite == "beyond_sqlite" {
        verdicts::case_verdicts(&raw_records)
    } else {
        let manifest = case_manifest(&options, &raw_records)?;
        verdicts::reduce_sqlite_verdicts(&raw_records, &manifest, warmup, repetitions)?
    };

    // Beyond-SQLite records are feature metadata with no timings, so that
    // suite has no latency ranking; every other suite must rank cleanly.
    let ranked = if options.suite == "beyond_sqlite" {
        Vec::new()
    } else {
        rank_cases(&raw_records)?
    };
    let evidence_versions = options
        .official_evidence
        .as_deref()
        .map(read_official_evidence_versions)
        .transpose()?;
    let measured_samples = raw_records
        .iter()
        .filter(|record| utils::is_measured(record))
        .count();
    let warmup_samples = raw_records
        .iter()
        .filter(|record| record.sample_role == "warmup")
        .count();
    let summary = SummaryJson {
        suite: options.suite.clone(),
        total_cases: verdicts.total(),
        passed_cases: verdicts.passed.len(),
        failed_cases: verdicts.failed.len(),
        skipped_cases: verdicts.skipped.len(),
        elapsed_ns: identity.measurement.elapsed_ns,
        measured_samples,
        warmup_samples,
        ranked_cases: ranked.len(),
        repetitions,
        warmup,
        measurement_boundary: crate::latency::MEASUREMENT_BOUNDARY.to_owned(),
        ranked_schema: crate::latency::RANKED_CSV_SCHEMA.to_owned(),
    };

    let summary_json = serde_json::to_string_pretty(&summary)? + "\n";
    let ranked_csv = ranked_csv(&ranked);
    let qualification = if options.suite == "sqlite_parity" {
        Some(build_sqlite_qualification(
            &summary,
            &raw_records,
            options.official_evidence.as_deref(),
        )?)
    } else {
        None
    };
    let report_block = render_report_block(
        &summary,
        &ranked,
        &raw_records,
        &options,
        evidence_versions.as_ref(),
        qualification.as_ref(),
        &identity,
    );
    let mut readme = fs::read_to_string(&options.readme)
        .with_context(|| format!("read README {}", options.readme.display()))?;
    readme = replace_block(&readme, REPORT_BEGIN, REPORT_END, &report_block);
    if let Some(qualification) = &qualification {
        readme = replace_block_if_present(
            &readme,
            BADGE_BEGIN,
            BADGE_END,
            &render_sqlite_badge(qualification),
        );
    }
    for (begin, end) in RETIRED_BLOCKS {
        readme = remove_block_if_present(&readme, begin, end);
    }

    let output_dir = &options.out_dir;
    let artifact_names = artifact_names_for_suite(&options.suite);
    let raw_out = output_dir.join(artifact_names.raw);
    let ranked_out = output_dir.join(artifact_names.ranked);
    let summary_out = output_dir.join(artifact_names.summary);
    let manifest_out = output_dir.join(artifact_names.manifest);
    let report_provenance_out = output_dir.join(artifact_names.report_provenance);

    let command_line = normalized_command_line();
    let mut output_files = BTreeMap::from([
        ("raw".to_owned(), raw_out.display().to_string()),
        ("summary".to_owned(), summary_out.display().to_string()),
        ("ranked".to_owned(), ranked_out.display().to_string()),
        (
            "report_provenance".to_owned(),
            report_provenance_out.display().to_string(),
        ),
    ]);
    if let Some(path) = &identity.parent.path {
        output_files.insert("run_provenance".to_owned(), path.clone());
    }
    let manifest = ManifestJson {
        schema_version: "redline-testing-manifest-v1".to_owned(),
        suite: options.suite.clone(),
        command_line: command_line.clone(),
        repetitions: summary.repetitions,
        warmup: summary.warmup,
        output_files,
    };
    let manifest_json = serde_json::to_string_pretty(&manifest)? + "\n";
    let svg_artifacts = build_svg_artifacts(&summary, &ranked, &raw_records, &options);

    let raw_sha256 = sha256_hex(&raw_text);
    let mut output_file_hashes = BTreeMap::from([
        (artifact_names.raw.to_owned(), raw_sha256.clone()),
        (artifact_names.summary.to_owned(), sha256_hex(&summary_json)),
        (artifact_names.ranked.to_owned(), sha256_hex(&ranked_csv)),
        (
            artifact_names.manifest.to_owned(),
            sha256_hex(&manifest_json),
        ),
    ]);
    // Only the blocks this report writes: README text outside them is not
    // report output.
    let readme_name = options.readme.display().to_string();
    for (begin, end) in [(REPORT_BEGIN, REPORT_END), (BADGE_BEGIN, BADGE_END)] {
        if let Some(block) = block_text(&readme, begin, end) {
            let marker = begin
                .trim_start_matches("<!-- ")
                .trim_end_matches(":begin -->");
            output_file_hashes.insert(format!("{readme_name}#{marker}"), sha256_hex(block));
        }
    }
    for artifact in &svg_artifacts {
        output_file_hashes.insert(
            artifact.path.display().to_string(),
            sha256_hex(&artifact.contents),
        );
    }
    let renderer = if options.check {
        committed_renderer(&report_provenance_out)
    } else {
        None
    };
    let renderer = match renderer {
        Some(renderer) => renderer,
        None => {
            let redline_testing_bin =
                std::env::current_exe().context("resolve current executable")?;
            Renderer {
                version: format!("redline-testing {}", env!("CARGO_PKG_VERSION")),
                binary_sha256: sha256_file(&redline_testing_bin)?,
            }
        }
    };
    let report_provenance = ReportProvenanceJson {
        schema_version: REPORT_PROVENANCE_SCHEMA,
        suite: options.suite.clone(),
        mode: identity.mode,
        note: identity.note(),
        measurement: identity.measurement.clone(),
        parent_run_provenance: identity.parent.clone(),
        run_evidence_sha256: identity.run_evidence_sha256.clone(),
        processed_evidence_sha256: identity.processed_evidence_sha256.clone(),
        raw_sha256,
        renderer,
        command_line,
        updated_date: options.updated_date.clone(),
        output_file_hashes,
    };
    let report_provenance_json = serde_json::to_string_pretty(&report_provenance)? + "\n";

    let rendered = RenderedReport {
        raw: raw_text,
        summary: summary_json,
        ranked: ranked_csv,
        readme,
        manifest: manifest_json,
        report_provenance: report_provenance_json,
    };

    if options.check {
        verify_existing(
            &options.input,
            &raw_out,
            &summary_out,
            &ranked_out,
            &manifest_out,
            &report_provenance_out,
            &options.readme,
            &rendered,
            &svg_artifacts,
        )?;
        return Ok(());
    }

    fs::create_dir_all(output_dir).with_context(|| format!("create {}", output_dir.display()))?;
    fs::write(&raw_out, rendered.raw)?;
    fs::write(&summary_out, rendered.summary)?;
    fs::write(&ranked_out, rendered.ranked)?;
    fs::write(&manifest_out, rendered.manifest)?;
    fs::write(&report_provenance_out, rendered.report_provenance)?;
    fs::write(&options.readme, rendered.readme)?;
    fs::write(
        output_dir.join("raw.jsonl.sha256"),
        format!("{}\n", sha256_file(&raw_out)?),
    )?;

    for artifact in svg_artifacts {
        write_text(&artifact.path, &artifact.contents)?;
    }
    if let Some(score_path) = &options.jankurai_score
        && score_path.exists()
    {
        let score_text = fs::read_to_string(score_path)
            .with_context(|| format!("read jankurai score {}", score_path.display()))?;
        if !score_text.trim().is_empty() {
            fs::write(output_dir.join("jankurai-score.txt"), score_text)?;
        }
    }

    Ok(())
}

/// The run's warmups and measured repetitions per case. A report from
/// official evidence states them; a local diagnostic may leave them to the
/// records, whose every case is then held to what the first executed case
/// shows.
fn sample_plan(options: &ReportOptions, records: &[RawRecord]) -> Result<(usize, usize)> {
    match (options.expected_warmup, options.expected_repetitions) {
        (Some(warmup), Some(repetitions)) => return Ok((warmup, repetitions)),
        _ if options.official_evidence.is_some() => bail!(
            "a report from official evidence needs --expected-warmup and --expected-repetitions"
        ),
        _ => {}
    }
    let first = records
        .iter()
        .find(|record| record.status != "skipped" && record.sample_role != "not_run")
        .map(|record| record.case_id.as_str());
    let of_first = || {
        records
            .iter()
            .filter(move |record| Some(record.case_id.as_str()) == first)
    };
    let warmup = options.expected_warmup.unwrap_or_else(|| {
        of_first()
            .filter(|record| record.sample_role == "warmup")
            .count()
    });
    let repetitions = options.expected_repetitions.unwrap_or_else(|| {
        of_first()
            .filter_map(|record| record.repetition_index)
            .max()
            .unwrap_or(1)
    });
    Ok((warmup, repetitions))
}

/// The cases the records must cover: as given, else the suite's compiled-in
/// corpus for official evidence, else (a local diagnostic, perhaps of a
/// narrowed run) the cases the records hold.
fn case_manifest(options: &ReportOptions, records: &[RawRecord]) -> Result<BTreeSet<String>> {
    if let Some(manifest) = &options.case_manifest {
        return Ok(manifest.clone());
    }
    if options.official_evidence.is_some() {
        return crate::sqlite_parity::manifest_case_ids(&options.suite);
    }
    Ok(records
        .iter()
        .map(|record| record.case_id.clone())
        .collect())
}

/// The renderer block of a committed report provenance, which `--check`
/// keeps: it names whichever build wrote the file.
fn committed_renderer(path: &Path) -> Option<Renderer> {
    let value: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    let renderer = value.get("renderer")?;
    Some(Renderer {
        version: renderer.get("version")?.as_str()?.to_owned(),
        binary_sha256: renderer.get("binary_sha256")?.as_str()?.to_owned(),
    })
}
