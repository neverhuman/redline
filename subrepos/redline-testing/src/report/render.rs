use std::collections::BTreeMap;

use anyhow::{Context, Result};

use super::evidence::{
    memory_peak_summary, memory_status_summary, suite_display_name, suite_subject,
};
use super::provenance::{ReportIdentity, ReportMode};
use super::qualification::counts_text;
use super::ratio::{format_ratio, format_summary_ratio, summarize};
use super::types::{
    DeviationKind, EvidenceVersions, Qualification, RankedCase, RawRecord, ReportOptions,
    SqliteQualification, SummaryJson,
};
use super::utils::{csv, is_measured, median};
use crate::latency::{
    CaseLatency, MEASUREMENT_BOUNDARY, MEASUREMENT_BOUNDARY_TEXT, RANKED_CSV_HEADER,
};

pub(crate) fn parse_raw_records(raw_text: &str) -> Result<Vec<RawRecord>> {
    let mut records = Vec::new();
    for (index, line) in raw_text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        records.push(
            serde_json::from_str(line)
                .with_context(|| format!("parse raw JSONL line {}", index.saturating_add(1)))?,
        );
    }
    Ok(records)
}

/// Per-case medians and latency ratios, slowest relative to SQLite first.
///
/// A measured case with a zero median on either side is an error: a missing
/// timing must never turn into a ratio, let alone a win.
pub(crate) fn rank_cases(records: &[RawRecord]) -> Result<Vec<RankedCase>> {
    let mut grouped = BTreeMap::<String, Vec<&RawRecord>>::new();
    for record in records.iter().filter(|record| is_measured(record)) {
        grouped
            .entry(record.case_id.clone())
            .or_default()
            .push(record);
    }
    let mut ranked = Vec::new();
    for (case_id, group) in grouped {
        let first = group[0];
        let sqlite_median_ns = median(group.iter().map(|record| record.reference_elapsed_ns));
        let redline_median_ns = median(group.iter().map(|record| record.target_elapsed_ns));
        let latency = CaseLatency::from_medians(sqlite_median_ns, redline_median_ns)
            .with_context(|| format!("rank case {case_id}"))?;
        ranked.push(RankedCase {
            case_id,
            name: first.name.clone(),
            case_file: first.case_file.clone(),
            priority: first.priority.clone(),
            profile: first.profile.clone(),
            category: first.category.clone(),
            sqlite_median_ns,
            redline_median_ns,
            latency_ratio: latency.latency_ratio,
            gap_pct: latency.gap_pct,
            below_resolution: latency.below_resolution,
            faster: latency.faster,
            samples: group.len(),
        });
    }
    ranked.sort_by(|left, right| {
        right
            .latency_ratio
            .total_cmp(&left.latency_ratio)
            .then_with(|| left.case_id.cmp(&right.case_id))
    });
    Ok(ranked)
}

pub(crate) fn ranked_csv(ranked: &[RankedCase]) -> String {
    let mut out = String::from(RANKED_CSV_HEADER);
    for (index, row) in ranked.iter().enumerate() {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{:.6},{:.6},{},{}\n",
            index.saturating_add(1),
            row.case_id,
            csv(&row.name),
            csv(&row.case_file),
            row.priority,
            row.profile,
            csv(&row.category),
            row.sqlite_median_ns,
            row.redline_median_ns,
            row.latency_ratio,
            row.gap_pct,
            row.below_resolution,
            row.samples
        ));
    }
    out
}

pub(crate) fn render_report_block(
    summary: &SummaryJson,
    ranked: &[RankedCase],
    raw_records: &[RawRecord],
    options: &ReportOptions,
    evidence_versions: Option<&EvidenceVersions>,
    qualification: Option<&SqliteQualification>,
    identity: &ReportIdentity,
) -> String {
    let suite_label = suite_display_name(&options.suite);
    let suite_subject = suite_subject(&options.suite);
    let latency = summarize(ranked);
    let latency_line = if latency.cases == 0 {
        "no latency ratio: no case has a passed measured sample to rank".to_owned()
    } else {
        format!(
            "median per-case latency ratio **{}** (RedlineDB/SQLite, lower is better), p95 **{}**, worst **{}**, faster **{}/{}**; **{}** cases have a SQLite median under 3 ms (`below_resolution`)",
            format_summary_ratio(latency.median_ratio),
            format_summary_ratio(latency.p95_ratio),
            format_summary_ratio(latency.worst_ratio),
            latency.faster,
            latency.cases,
            latency.below_resolution
        )
    };
    let lane = evidence_versions.map_or("unrecorded (no official evidence)", |versions| {
        versions.lane.as_str()
    });
    let boundary_line = format!(
        "**Measurement boundary:** {MEASUREMENT_BOUNDARY_TEXT} (`{MEASUREMENT_BOUNDARY}`: process spawn, case execution and output collection; fixture writes untimed); lane={lane}; not a tuned benchmark.\n\n"
    );
    let mut block = String::new();
    if let Some(qualification) = qualification {
        block.push_str(&render_sqlite_scope(
            qualification,
            identity,
            &options.updated_date,
        ));
    } else {
        block.push_str(&format!(
            "**{} coverage:** **{} / {}** {} passed in CI. Failed: **{}**. Skipped: **{}**. Updated {}.\n\n",
            suite_label,
            summary.passed_cases,
            summary.total_cases,
            suite_subject,
            summary.failed_cases,
            summary.skipped_cases,
            options.updated_date
        ));
    }
    match options.suite.as_str() {
        "beyond_sqlite" => {
            block.push_str(&format!(
                "**{} progress:** coverage **{:.2}%**, promoted reference features **{}**, manifest backlog **{}**.\n\n",
                suite_label,
                if summary.total_cases == 0 {
                    0.0
                } else {
                    summary.passed_cases as f64 / summary.total_cases as f64 * 100.0
                },
                summary.passed_cases,
                summary.skipped_cases
            ));
        }
        "memory" => {
            let memory_status = memory_status_summary(raw_records);
            let memory_peaks = memory_peak_summary(raw_records)
                .unwrap_or_else(|| "no RSS samples were captured".to_owned());
            block.push_str(&format!(
                "**{suite_label} latency:** {latency_line}. RSS sampling: **{memory_status}**. {memory_peaks}.\n\n"
            ));
            block.push_str(&boundary_line);
        }
        _ => {
            block.push_str(&format!("**{suite_label} latency:** {latency_line}.\n\n"));
            block.push_str(&boundary_line);
        }
    }
    if let Some(evidence_versions) = evidence_versions {
        block.push_str(&format!(
            "**Benchmark metadata:** RedlineDB target version **{}**, SQLite reference version **{}**, redline-testing runner version **{}**.\n\n",
            evidence_versions.target_version,
            evidence_versions.sqlite_version,
            evidence_versions.runner_version
        ));
    }
    if let Some(plot) = &options.plot {
        block.push_str(&format!(
            "![{} latency ratio plot]({})\n\n",
            suite_label,
            plot.display()
        ));
    }
    if let Some(plot) = &options.performance_histogram_plot {
        block.push_str(&format!(
            "![{} performance distribution]({})\n\n",
            suite_label,
            plot.display()
        ));
    }
    if let Some(plot) = &options.median_test_performance_plot {
        block.push_str(&format!(
            "![{} median ratio]({})\n\n",
            suite_label,
            plot.display()
        ));
    }
    let table_id = format!("{}-ranked-table", options.suite.replace('_', "-"));
    let table_summary = if options.suite == "beyond_sqlite" {
        "Full ranked feature table"
    } else {
        "Slowest 25 cases by latency ratio (every case is in ranked.csv)"
    };
    block.push_str(&format!(
        "<details id=\"{}\">\n<summary>{}</summary>\n\n",
        table_id, table_summary
    ));
    block.push_str("| Rank | Case | Priority | Profile | Category | SQLite median ns | RedlineDB median ns | Ratio | Gap |\n");
    block.push_str("| ---: | --- | --- | --- | --- | ---: | ---: | ---: | ---: |\n");
    for (index, row) in ranked.iter().take(25).enumerate() {
        block.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {:+.2}% |\n",
            index + 1,
            row.name,
            row.priority,
            row.profile,
            row.category,
            row.sqlite_median_ns,
            row.redline_median_ns,
            format_ratio(row.latency_ratio),
            row.gap_pct
        ));
    }
    block.push_str("\n</details>\n");
    block
}

const SQLITE_BADGE_LABEL: &str = "SQLite SQL/CLI corpus";

/// The first lines of the `sqlite_parity` block: corpus and oracle, the fixed
/// scope sentence, the evidence behind the counts, and the declared deviations.
fn render_sqlite_scope(
    qualification: &SqliteQualification,
    identity: &ReportIdentity,
    updated_date: &str,
) -> String {
    let q = qualification;
    let oracle = q.oracle_version.as_deref().map_or_else(
        || "sqlite3 shell, version unrecorded".to_owned(),
        |version| format!("sqlite3 {version} shell"),
    );
    let mut out = format!(
        "**{SQLITE_BADGE_LABEL}** (redline-testing `{}`, {oracle}): **{} / {}** cases passed, **{}** failed, **{}** skipped. Updated {updated_date}.\n\n",
        q.corpus_id, q.passed, q.total, q.failed, q.skipped
    );
    out.push_str(&format!(
        "**Scope** (`{}`): each case runs one SQL or dot-command script through the `redlinedb` and `sqlite3` shells and compares their output and exit status. It does not test C ABI semantics, the database file format, or prepared-statement state.\n\n",
        q.surface
    ));
    let counts = counts_text([q.total, q.passed, q.failed, q.skipped]);
    match (&q.qualification, &q.run_id) {
        (Qualification::Qualified, Some(run_id)) => out.push_str(&format!(
            "**Evidence:** qualified: official evidence run `{}` records the same {counts}.",
            short(run_id)
        )),
        (Qualification::Qualified, None) => {
            out.push_str(&format!("**Evidence:** qualified: {counts}."));
        }
        (Qualification::Unqualified(reason), _) => {
            out.push_str(&format!("**Evidence:** unqualified: {reason}."));
        }
    }
    if q.run_id.is_some() {
        let runner = match (&q.runner_version, &q.runner_sha256) {
            (Some(version), Some(sha)) => {
                format!("from {version} (runner SHA-256 `{}`)", short(sha))
            }
            (Some(version), None) => format!("from {version}"),
            (None, Some(sha)) => format!("from runner SHA-256 `{}`", short(sha)),
            (None, None) => "from an unrecorded runner".to_owned(),
        };
        let oracle_binary = q
            .oracle_binary_sha256
            .as_deref()
            .map_or_else(String::new, |sha| {
                format!(" (binary SHA-256 `{}`)", short(sha))
            });
        out.push_str(&format!(
            " Corpus `{}` {runner}, corpus SHA-256 {}; oracle sqlite3 {}{oracle_binary}, build stamp {}.",
            q.corpus_id,
            recorded(q.corpus_sha256.as_deref()),
            q.oracle_version.as_deref().unwrap_or("unrecorded"),
            recorded(q.oracle_build_id.as_deref()),
        ));
    }
    out.push_str(&run_provenance_sentence(identity));
    out.push_str("\n\n");
    if q.declared_deviations.is_empty() {
        out.push_str("**Declared deviations:** none among the cases in this run.\n\n");
    }
    for (kind, heading) in DEVIATION_HEADINGS {
        let declared = q
            .declared_deviations
            .iter()
            .filter(|deviation| deviation.kind == kind)
            .collect::<Vec<_>>();
        if declared.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "**{} ({}):** {}\n\n",
            heading.0,
            declared.len(),
            heading.1
        ));
        for deviation in declared {
            out.push_str(&format!(
                "- `{}` {}: {}\n",
                deviation.case_id, deviation.name, deviation.reason
            ));
        }
        out.push('\n');
    }
    out
}

/// The report groups declared cases by kind, in this order.
const DEVIATION_HEADINGS: [(DeviationKind, (&str, &str)); 3] = [
    (
        DeviationKind::StandIn,
        (
            "Declared deviations",
            "these cases pass, but RedlineDB produces the compared output without the SQLite feature behind it.",
        ),
    ),
    (
        DeviationKind::SharedRejection,
        (
            "Declared shared rejections",
            "the pinned sqlite3 build lacks the feature, so these cases declare its error; a pass means RedlineDB rejected the statement too, not that the feature works.",
        ),
    ),
    (
        DeviationKind::OracleBuild,
        (
            "Declared oracle-build deviations",
            "these cases were written for a different SQLite build; against the pinned sqlite3 they check what the reason states.",
        ),
    ),
];

/// Where the measured source identity comes from, after the Evidence line.
fn run_provenance_sentence(identity: &ReportIdentity) -> String {
    match (identity.mode, &identity.measurement.run) {
        (ReportMode::Official, Some(run)) => format!(
            " Run provenance {}: source tree `{}` ({}), source inputs `{}`, assertion policy `{}`.",
            recorded(identity.parent.sha256.as_deref()),
            short(&run.source_tree),
            if run.source_dirty { "dirty" } else { "clean" },
            short(&run.source_inputs_sha256),
            short(&run.assertion_policy_sha256),
        ),
        (ReportMode::Historical, _) => format!(
            " Historical run: it predates run provenance, so its source tree is unrecorded, and its run provenance{} was not retained.",
            identity
                .parent
                .sha256
                .as_deref()
                .map_or_else(String::new, |sha| format!(" `{}`", short(sha)))
        ),
        _ => String::new(),
    }
}

/// The first 12 characters of an identifier's first line.
fn short(id: &str) -> String {
    id.lines().next().unwrap_or("").chars().take(12).collect()
}

fn recorded(id: Option<&str>) -> String {
    id.map_or_else(|| "unrecorded".to_owned(), |id| format!("`{}`", short(id)))
}

/// The README badge for one `sqlite_parity` run. Green needs a qualified run
/// with no failures and no skips; the alt text always states the scope.
pub(crate) fn render_sqlite_badge(qualification: &SqliteQualification) -> String {
    let q = qualification;
    let (message, color, alt) = match &q.qualification {
        Qualification::Unqualified(reason) => (
            "unqualified".to_owned(),
            "lightgrey",
            format!("{SQLITE_BADGE_LABEL}: unqualified ({reason}); not full SQLite compatibility"),
        ),
        Qualification::Qualified => {
            let oracle = q.oracle_version.as_deref().unwrap_or("unrecorded");
            let color = if q.failed > 0 {
                "red"
            } else if q.skipped > 0 {
                "orange"
            } else {
                "brightgreen"
            };
            let deviations = match q.deviation_count {
                1 => "1 declared deviation".to_owned(),
                count => format!("{count} declared deviations"),
            };
            (
                format!(
                    "{}/{} \u{b7} {} failed \u{b7} {} skipped \u{b7} {oracle}",
                    q.passed, q.total, q.failed, q.skipped
                ),
                color,
                format!(
                    "{SQLITE_BADGE_LABEL}: {}/{} cases passed, {} failed, {} skipped against the sqlite3 {oracle} shell; {deviations}; not full SQLite compatibility",
                    q.passed, q.total, q.failed, q.skipped
                ),
            )
        }
    };
    format!(
        "  <a href=\"#sqlite-parity-status\"><img src=\"https://img.shields.io/badge/{}-{}-{color}\" alt=\"{}\"></a>",
        shields_text(SQLITE_BADGE_LABEL),
        shields_text(&message),
        html_attribute(&alt),
    )
}

/// Escapes one shields.io static-badge path segment: `-` and `_` double, and
/// everything but ASCII alphanumerics, `.` and `~` is percent-encoded.
fn shields_text(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '-' => out.push_str("--"),
            '_' => out.push_str("__"),
            'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '~' => out.push(ch),
            _ => {
                let mut buffer = [0u8; 4];
                for byte in ch.encode_utf8(&mut buffer).bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    out
}

fn html_attribute(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The text between `begin` and `end` when both are present in order.
pub(crate) fn block_text<'a>(text: &'a str, begin: &str, end: &str) -> Option<&'a str> {
    let start = text.find(begin)? + begin.len();
    let stop = text.find(end)?;
    (start <= stop).then(|| &text[start..stop])
}

/// Deletes a generated block, markers included, when both markers are
/// present in order; otherwise returns the text unchanged.
pub(crate) fn remove_block_if_present(text: &str, begin: &str, end: &str) -> String {
    match (text.find(begin), text.find(end)) {
        (Some(begin_index), Some(end_index)) if begin_index < end_index => {
            let mut stop = end_index + end.len();
            if text[stop..].starts_with('\n') {
                stop += 1;
            }
            format!("{}{}", &text[..begin_index], &text[stop..])
        }
        _ => text.to_owned(),
    }
}

pub(crate) fn replace_block_if_present(
    text: &str,
    begin: &str,
    end: &str,
    replacement: &str,
) -> String {
    match (text.find(begin), text.find(end)) {
        (Some(begin_index), Some(end_index)) if begin_index < end_index => {
            replace_block(text, begin, end, replacement)
        }
        _ => text.to_owned(),
    }
}

pub(crate) fn replace_block(text: &str, begin: &str, end: &str, replacement: &str) -> String {
    match (text.find(begin), text.find(end)) {
        (Some(begin_index), Some(end_index)) if begin_index < end_index => {
            let mut result = String::new();
            result.push_str(&text[..begin_index + begin.len()]);
            result.push('\n');
            result.push_str(replacement);
            result.push_str(&text[end_index..]);
            result
        }
        _ => format!("{text}\n{begin}\n{replacement}\n{end}\n"),
    }
}
