//! Builds the `SqliteQualification` the SQLite badge and report block render
//! (L-01/SQ-01): what one `sqlite_parity` run measured, against which oracle,
//! and whether validated official evidence stands behind the counts.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;

use super::evidence::official_suite_entry;
use super::types::{DeclaredDeviation, Qualification, RawRecord, SqliteQualification, SummaryJson};
use super::utils::sha256_hex;

pub(crate) const SURFACE: &str = "sqlite_sql_cli";
const SUITE: &str = "sqlite_parity";
const DEVIATIONS_SCHEMA: &str = "redline-testing-sqlite-declared-deviations-v2";
const DECLARED_DEVIATIONS: &str =
    include_str!("../../metadata/sqlite_parity/declared-deviations.json");

#[derive(Deserialize)]
struct DeviationFile {
    schema_version: String,
    deviations: Vec<DeclaredDeviation>,
}

/// Every declared deviation, in file order.
pub(crate) fn declared_deviations() -> Result<Vec<DeclaredDeviation>> {
    let file: DeviationFile = serde_json::from_str(DECLARED_DEVIATIONS)
        .context("parse metadata/sqlite_parity/declared-deviations.json")?;
    if file.schema_version != DEVIATIONS_SCHEMA {
        bail!(
            "declared-deviations.json schema_version {:?}, expected {DEVIATIONS_SCHEMA:?}",
            file.schema_version
        );
    }
    Ok(file.deviations)
}

/// The qualification of a `sqlite_parity` run. Without official evidence it
/// is `Unqualified`; the evidence binding to the raw bytes was already
/// checked by `validate_official_evidence_binding`.
pub(crate) fn build_sqlite_qualification(
    summary: &SummaryJson,
    raw_records: &[RawRecord],
    official_evidence: Option<&Path>,
) -> Result<SqliteQualification> {
    let run_cases = raw_records
        .iter()
        .map(|record| record.case_id.as_str())
        .collect::<BTreeSet<_>>();
    let declared_deviations = declared_deviations()?
        .into_iter()
        .filter(|deviation| run_cases.contains(deviation.case_id.as_str()))
        .collect::<Vec<_>>();
    let mut qualification = SqliteQualification {
        surface: SURFACE,
        corpus_id: SUITE.to_owned(),
        runner_version: None,
        runner_sha256: None,
        corpus_sha256: None,
        oracle_version: None,
        oracle_build_id: None,
        oracle_binary_sha256: None,
        run_id: None,
        total: summary.total_cases,
        passed: summary.passed_cases,
        failed: summary.failed_cases,
        skipped: summary.skipped_cases,
        deviation_count: declared_deviations.len(),
        declared_deviations,
        qualification: Qualification::Unqualified("no official evidence".to_owned()),
    };
    let Some(path) = official_evidence else {
        return Ok(qualification);
    };
    let text = fs::read_to_string(path)
        .with_context(|| format!("read official evidence {}", path.display()))?;
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("parse official evidence {}", path.display()))?;
    // Processed evidence nests the run's own official-evidence.json and
    // records that file's hash; the run file itself is its own identity.
    let run = value.get("official_evidence").unwrap_or(&value);
    qualification.run_id =
        Some(text_at(&value, &["source_sha256"]).unwrap_or_else(|| sha256_hex(&text)));
    qualification.runner_version = text_at(run, &["runner", "version"]);
    qualification.runner_sha256 = text_at(run, &["runner", "binary_sha256"]);
    qualification.corpus_sha256 = text_at(run, &["corpus_sha256"]);
    qualification.oracle_version =
        text_at(run, &["sqlite", "version"]).and_then(|version| release_of(&version));
    qualification.oracle_binary_sha256 = text_at(run, &["sqlite", "sha256"]);
    qualification.oracle_build_id = text_at(run, &["oracle_build_stamp"]);
    qualification.qualification = qualify(&value, run, &qualification);
    Ok(qualification)
}

fn qualify(value: &Value, run: &Value, facts: &SqliteQualification) -> Qualification {
    let unqualified = |reason: String| Qualification::Unqualified(reason);
    let raw = [facts.total, facts.passed, facts.failed, facts.skipped];
    let status = text_at(value, &["status"]).or_else(|| text_at(run, &["status"]));
    match status.as_deref() {
        None => return unqualified("official evidence records no run status".to_owned()),
        Some("passed") if facts.failed > 0 => {
            return unqualified(format!(
                "official evidence status is passed, but the raw results have {} failed",
                facts.failed
            ));
        }
        Some("passed" | "failed") => {}
        Some(other) => return unqualified(format!("official evidence status is {other:?}")),
    }
    let Some(recorded) = recorded_counts(value, run) else {
        return unqualified(format!("official evidence records no {SUITE} counts"));
    };
    if recorded != raw {
        return unqualified(format!(
            "official evidence records {}, but the raw results have {}",
            counts_text(recorded),
            counts_text(raw)
        ));
    }
    if facts.oracle_version.is_none() {
        return unqualified("official evidence records no SQLite oracle version".to_owned());
    }
    Qualification::Qualified
}

/// (total, passed, failed, skipped) as the run recorded them for the suite.
fn recorded_counts(value: &Value, run: &Value) -> Option<[usize; 4]> {
    let entry = value
        .get("suite_summaries")
        .and_then(|summaries| summaries.get(SUITE))
        .or_else(|| official_suite_entry(run, SUITE))?;
    let count = |key: &str| {
        entry
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|count| usize::try_from(count).ok())
    };
    Some([
        count("total")?,
        count("passed")?,
        count("failed")?,
        count("skipped")?,
    ])
}

pub(crate) fn counts_text([total, passed, failed, skipped]: [usize; 4]) -> String {
    format!("{total} total, {passed} passed, {failed} failed, {skipped} skipped")
}

fn text_at(value: &Value, path: &[&str]) -> Option<String> {
    path.iter()
        .try_fold(value, |value, key| value.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty() && *text != "<unknown>")
        .map(str::to_owned)
}

/// The SQLite release in a `sqlite3 --version` line: its first dotted number.
fn release_of(version: &str) -> Option<String> {
    version
        .split_whitespace()
        .find(|token| {
            token.contains('.')
                && token.starts_with(|ch: char| ch.is_ascii_digit())
                && token.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
        })
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{declared_deviations, release_of};
    use crate::report::types::DeviationKind;

    #[test]
    fn declared_deviations_name_real_corpus_cases() {
        let corpus = crate::sqlite_parity::all_cases()
            .expect("sqlite_parity corpus")
            .into_iter()
            .map(|case| (case.display_id(), case.name))
            .collect::<Vec<_>>();
        let deviations = declared_deviations().expect("declared deviations");
        let ids = deviations
            .iter()
            .map(|deviation| deviation.case_id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), deviations.len(), "duplicate case id");
        for deviation in &deviations {
            assert!(
                corpus
                    .iter()
                    .any(|(id, name)| *id == deviation.case_id && *name == deviation.name),
                "{} {} is not a sqlite_parity case",
                deviation.case_id,
                deviation.name
            );
            assert!(!deviation.reason.trim().is_empty(), "{}", deviation.case_id);
        }
        // The fts5, highlight, rtree and dbstat stand-ins are declared.
        for id in ["00093", "00094", "00095", "00096"] {
            assert!(ids.contains(id), "{id} must be declared");
        }
        // So are the cases the pinned oracle build cannot express: the
        // shared soundex() and UPDATE/DELETE ... LIMIT rejections, and the
        // median()-is-absent case the pinned build contradicts.
        for id in ["00219", "00220", "10546", "11437", "11438", "11439"] {
            assert!(ids.contains(id), "{id} must be declared");
        }
        let kind_of = |id: &str| {
            deviations
                .iter()
                .find(|deviation| deviation.case_id == id)
                .map(|deviation| deviation.kind)
        };
        for id in ["00219", "00220", "11437", "11438", "11439"] {
            assert_eq!(kind_of(id), Some(DeviationKind::SharedRejection), "{id}");
        }
        assert_eq!(kind_of("10546"), Some(DeviationKind::OracleBuild));
        assert_eq!(kind_of("00093"), Some(DeviationKind::StandIn));
    }

    #[test]
    fn declared_shared_rejections_expect_the_rejection() {
        // A shared rejection passes only on the error it declares, so the
        // case must expect a failure and name text from it.
        let corpus = crate::sqlite_parity::all_cases().expect("sqlite_parity corpus");
        for deviation in declared_deviations().expect("declared deviations") {
            if deviation.kind != DeviationKind::SharedRejection {
                continue;
            }
            let case = corpus
                .iter()
                .find(|case| case.display_id() == deviation.case_id)
                .expect("declared case is in the corpus");
            assert_ne!(case.expected_exit, 0, "{}", deviation.case_id);
            assert!(
                case.expected_stderr_contains
                    .iter()
                    .any(|needle| !needle.is_empty()),
                "{} declares no stderr fragment",
                deviation.case_id
            );
        }
    }

    #[test]
    fn oracle_release_is_the_first_dotted_number() {
        assert_eq!(
            release_of("3.53.1 2026-05-05 10:34:17 c88b22011a54 (64-bit)").as_deref(),
            Some("3.53.1")
        );
        assert_eq!(
            release_of("sqlite evidence 3.44.0").as_deref(),
            Some("3.44.0")
        );
        assert_eq!(release_of("<unknown>"), None);
    }
}
