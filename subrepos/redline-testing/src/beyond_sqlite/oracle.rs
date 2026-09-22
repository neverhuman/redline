//! Executable beyond-SQLite oracle.
//!
//! Loads cases from `corpus/beyond_sqlite/generated_manifest.json`, runs each
//! against the resolved Postgres reference (see `engine::resolve`), normalizes
//! the captured output via the per-case `Normalizer` pipeline, and validates
//! that reference and target shells agree.
//!
//! When Postgres is unavailable the cases are emitted as `skipped` records
//! with a diagnostic. The required qualification gate rejects missing target
//! comparisons. The separate SQLite-only suite does not require PostgreSQL.
//!
//! Two compare lanes are emitted per case:
//!   1. **Reference self-compare** (`profile: beyond_sqlite_oracle`) — runs
//!      psql against the case twice and asserts identical output. This is the
//!      ship gate: a case can only sit in the published corpus if psql agrees
//!      with itself. Failures here mean the case is non-deterministic and
//!      must be repaired before qualification can pass.
//!   2. **Target compare** (`profile: beyond_sqlite_target`, opt-in via
//!      `RunCasesOptions::target_bin`) — when a target binary is supplied,
//!      drives it with the same stdin (plus a SQLite-style formatting
//!      preamble) and compares normalized output to psql. Failures here are
//!      engine gaps in redlinedb / target-under-test; these are exported to
//!      the gap ledger for downstream agents to close.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

use super::case::{BeyondCase, CompareMode};
use super::engine::{PostgresReference, ResolveOutcome, invoke_psql, resolve};
use super::normalize::apply_chain;

pub(crate) const MANIFEST: &str =
    include_str!("../../corpus/beyond_sqlite/generated_manifest.json");

/// SQLite-shell formatting preamble. Mirrors the `.mode list / .nullvalue NULL
/// / .separator |` setup baked into per-case sqlite_parity stdin so target
/// shell output is byte-comparable with psql's `-A -t -F | -P null=NULL`
/// formatting through the normalizer pipeline.
///
/// `PRAGMA case_sensitive_like = 1` flips LIKE from its SQLite default
/// (ASCII-fold) to Postgres' SQL-standard case-sensitive behaviour. The
/// beyond-SQLite oracle compares against PG, so case-sensitive LIKE is
/// what we want for cases like `LIKE_VS_ILIKE_ASCII`. ILIKE is
/// independent of this pragma and continues to fold case.
const SQLITE_FORMATTING_PREAMBLE: &str =
    ".mode list\n.headers off\n.separator |\n.nullvalue NULL\nPRAGMA case_sensitive_like = 1;\n";

#[derive(Debug, Clone, Default)]
pub struct RunCasesOptions {
    /// Target binary to validate against the psql reference. When `None` the
    /// oracle only runs the psql self-compare ship gate.
    pub target_bin: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct OracleSummary {
    pub reference: Option<super::engine::ReferenceIdentity>,
    pub total: usize,
    pub passed: usize,
    pub skipped_unavailable: usize,
    pub skipped_feature_missing: usize,
    pub failed: usize,
    /// Target-compare lane counts. Always zero when `target_bin` is None.
    pub target_total: usize,
    pub target_passed: usize,
    pub target_failed: usize,
    pub target_skipped: usize,
    /// The same target lane split by `metadata/beyond_sqlite/skip-list.toml`.
    /// In-scope is the number the project drives; deferred is the number it
    /// has decided not to. Reporting only their sum is what let "97% covered"
    /// and "103 failing" appear in the same artifact.
    pub in_scope_total: usize,
    pub in_scope_passed: usize,
    pub in_scope_failed: usize,
    pub deferred_total: usize,
    pub deferred_passed: usize,
    pub deferred_failed: usize,
    /// Deferred cases that pass. A deferral that has come true is a bug in the
    /// skip-list, not a result, so it is named rather than counted.
    pub deferred_passing_cases: Vec<String>,
}

pub fn load_cases() -> Result<Vec<BeyondCase>> {
    parse_cases(MANIFEST)
}

fn parse_cases(manifest: &str) -> Result<Vec<BeyondCase>> {
    let cases: Vec<BeyondCase> = serde_json::from_str(manifest)
        .context("parse corpus/beyond_sqlite/generated_manifest.json")?;
    let mut ids = std::collections::BTreeSet::new();
    anyhow::ensure!(!cases.is_empty(), "Postgres corpus is empty");
    for case in &cases {
        anyhow::ensure!(ids.insert(case.id), "duplicate Postgres case {}", case.id);
        anyhow::ensure!(
            case.script.is_none()
                && case.files.is_empty()
                && case.requires_engine_features.is_empty(),
            "disabled required comparison in case {}",
            case.id
        );
    }
    Ok(cases)
}

/// Backwards-compat wrapper: run the psql self-compare ship-gate only,
/// no target-vs-reference lane. Retained for tests and external callers.
#[allow(dead_code)]
pub fn run_cases() -> Result<(OracleSummary, Vec<CaseOutcome>)> {
    run_cases_with(RunCasesOptions::default())
}

pub fn run_cases_with(options: RunCasesOptions) -> Result<(OracleSummary, Vec<CaseOutcome>)> {
    let cases = load_cases()?;
    let deferred_ids = super::skip_list::deferred_case_ids()?;
    let reference = resolve();
    let mut summary = OracleSummary {
        reference: None,
        total: cases.len(),
        passed: 0,
        skipped_unavailable: 0,
        skipped_feature_missing: 0,
        failed: 0,
        target_total: 0,
        target_passed: 0,
        target_failed: 0,
        target_skipped: 0,
        in_scope_total: 0,
        in_scope_passed: 0,
        in_scope_failed: 0,
        deferred_total: 0,
        deferred_passed: 0,
        deferred_failed: 0,
        deferred_passing_cases: Vec::new(),
    };
    let mut outcomes = Vec::with_capacity(cases.len());
    let pg = match reference {
        ResolveOutcome::Configured(pg) => pg,
        ResolveOutcome::Unavailable { reason } => {
            // All cases skip with the same reason; we still emit per-case
            // outcomes for consumer-facing reports.
            for case in cases {
                summary.skipped_unavailable += 1;
                outcomes.push(CaseOutcome {
                    case_id: case.id,
                    name: case.name.clone(),
                    feature_rank: case.feature_rank,
                    category: case.category.clone(),
                    status: "skipped".to_owned(),
                    diagnostic: Some(format!("postgres unavailable: {reason}")),
                    target: None,
                });
            }
            return Ok((summary, outcomes));
        }
    };

    summary.reference = Some(super::engine::identity(&pg)?);
    for case in &cases {
        let mut outcome = run_one_case(case, &pg);
        // Only attempt target compare when self-compare passed AND a target
        // binary was supplied. Otherwise the target lane is meaningless (a
        // failing reference means the case is unstable).
        if outcome.status == "passed"
            && let Some(target_bin) = options.target_bin.as_deref()
        {
            let target_outcome = run_one_case_against_target(case, &pg, target_bin);
            outcome.target = Some(target_outcome);
        }
        match outcome.status.as_str() {
            "passed" => summary.passed += 1,
            "skipped" => {
                if outcome
                    .diagnostic
                    .as_deref()
                    .unwrap_or("")
                    .contains("feature not advertised")
                {
                    summary.skipped_feature_missing += 1;
                } else {
                    summary.skipped_unavailable += 1;
                }
            }
            "failed" => summary.failed += 1,
            _ => {}
        }
        if let Some(target) = outcome.target.as_ref() {
            summary.target_total += 1;
            let case_id = format!("BEYOND-CASE-{:05}", outcome.case_id);
            let deferred = deferred_ids.contains(&case_id);
            if deferred {
                summary.deferred_total += 1;
            } else {
                summary.in_scope_total += 1;
            }
            match target.status.as_str() {
                "passed" => {
                    summary.target_passed += 1;
                    if deferred {
                        summary.deferred_passed += 1;
                        summary.deferred_passing_cases.push(case_id);
                    } else {
                        summary.in_scope_passed += 1;
                    }
                }
                "failed" => {
                    summary.target_failed += 1;
                    if deferred {
                        summary.deferred_failed += 1;
                    } else {
                        summary.in_scope_failed += 1;
                    }
                }
                _ => summary.target_skipped += 1,
            }
        }
        outcomes.push(outcome);
    }
    Ok((summary, outcomes))
}

#[derive(Debug, Clone)]
pub struct CaseOutcome {
    pub case_id: u64,
    pub name: String,
    pub feature_rank: usize,
    pub category: String,
    pub status: String,
    pub diagnostic: Option<String>,
    /// Target-vs-reference compare result, populated only when
    /// `RunCasesOptions::target_bin` is supplied AND the self-compare passes.
    pub target: Option<TargetOutcome>,
}

#[derive(Debug, Clone)]
pub struct TargetOutcome {
    /// "passed" | "failed" | "skipped".
    pub status: String,
    /// Diagnostic explaining a failure / skip; `None` on pass.
    pub diagnostic: Option<String>,
    /// Reference exit code (psql).
    pub reference_exit: i32,
    /// Target exit code (None if invocation errored before exit).
    pub target_exit: Option<i32>,
    /// Reference normalized stdout (truncated for ledger sanity).
    pub reference_stdout: String,
    /// Target normalized stdout (truncated for ledger sanity).
    pub target_stdout: String,
    /// Target stderr first line (helpful for engine-error triage).
    pub target_stderr_head: String,
    pub reference_elapsed_ns: u128,
    pub target_elapsed_ns: u128,
}

fn run_one_case(case: &BeyondCase, pg: &PostgresReference) -> CaseOutcome {
    // The "reference ↔ reference" self-compare runs the case stdin twice
    // against the same psql connection (different sessions). For now the
    // ship contract is: every shipping case must produce identical output
    // on two consecutive psql invocations.
    let timeout = Duration::from_millis(case.timeout_ms().into());
    let stdin = assembled_stdin(case);
    let first = match invoke_psql(&pg.bin, &pg.connection, &stdin, &case.pg_settings, timeout) {
        Ok(out) => out,
        Err(err) => {
            return CaseOutcome {
                case_id: case.id,
                name: case.name.clone(),
                feature_rank: case.feature_rank,
                category: case.category.clone(),
                status: "skipped".to_owned(),
                diagnostic: Some(format!("reference invocation error: {err}")),
                target: None,
            };
        }
    };
    let second = match invoke_psql(&pg.bin, &pg.connection, &stdin, &case.pg_settings, timeout) {
        Ok(out) => out,
        Err(err) => {
            return CaseOutcome {
                case_id: case.id,
                name: case.name.clone(),
                feature_rank: case.feature_rank,
                category: case.category.clone(),
                status: "skipped".to_owned(),
                diagnostic: Some(format!("second reference invocation error: {err}")),
                target: None,
            };
        }
    };
    let ref_norm = normalize_for_compare(&first.stdout, case);
    let tgt_norm = normalize_for_compare(&second.stdout, case);
    if first.exit_code == case.expected_reference_exit
        && second.exit_code == case.expected_reference_exit
        && ref_norm == tgt_norm
        && first.stderr == second.stderr
    {
        CaseOutcome {
            case_id: case.id,
            name: case.name.clone(),
            feature_rank: case.feature_rank,
            category: case.category.clone(),
            status: "passed".to_owned(),
            diagnostic: None,
            target: None,
        }
    } else {
        CaseOutcome {
            case_id: case.id,
            name: case.name.clone(),
            feature_rank: case.feature_rank,
            category: case.category.clone(),
            status: "failed".to_owned(),
            diagnostic: Some(format!(
                "reference self-compare mismatch: exits=({}, {}) stdout=({:?}, {:?}) stderr=({:?}, {:?})",
                first.exit_code,
                second.exit_code,
                truncate(&ref_norm, 256),
                truncate(&tgt_norm, 256),
                truncate(&first.stderr, 256),
                truncate(&second.stderr, 256)
            )),
            target: None,
        }
    }
}

/// Drive the target shell with the case stdin (prefixed with a SQLite-style
/// formatting preamble) and compare normalized output to a fresh psql run
/// against the same reference. The reference run is repeated here (rather
/// than reusing `run_one_case`'s result) so the comparison is byte-exact
/// against the reference output captured at the same point in time.
fn run_one_case_against_target(
    case: &BeyondCase,
    pg: &PostgresReference,
    target_bin: &Path,
) -> TargetOutcome {
    let timeout = Duration::from_millis(case.timeout_ms().into());
    let stdin = assembled_stdin(case);

    let ref_started = Instant::now();
    let reference = match invoke_psql(&pg.bin, &pg.connection, &stdin, &case.pg_settings, timeout) {
        Ok(out) => out,
        Err(err) => {
            return TargetOutcome {
                status: "skipped".to_owned(),
                diagnostic: Some(format!("reference re-invocation error: {err}")),
                reference_exit: -1,
                target_exit: None,
                reference_stdout: String::new(),
                target_stdout: String::new(),
                target_stderr_head: String::new(),
                reference_elapsed_ns: ref_started.elapsed().as_nanos(),
                target_elapsed_ns: 0,
            };
        }
    };
    let reference_elapsed_ns = ref_started.elapsed().as_nanos();

    let target_stdin = format!("{SQLITE_FORMATTING_PREAMBLE}{stdin}");
    let target_started = Instant::now();
    let target = match invoke_target(target_bin, &target_stdin, timeout) {
        Ok(out) => out,
        Err(err) => {
            return TargetOutcome {
                status: "skipped".to_owned(),
                diagnostic: Some(format!("target invocation error: {err}")),
                reference_exit: reference.exit_code,
                target_exit: None,
                reference_stdout: truncate(&reference.stdout, 1024),
                target_stdout: String::new(),
                target_stderr_head: String::new(),
                reference_elapsed_ns,
                target_elapsed_ns: target_started.elapsed().as_nanos(),
            };
        }
    };
    let target_elapsed_ns = target_started.elapsed().as_nanos();

    let ref_norm = normalize_for_compare(&reference.stdout, case);
    let tgt_norm = normalize_for_compare(&target.stdout, case);
    let target_stderr_head = first_nonempty_line(&target.stderr);

    if reference.exit_code != case.expected_reference_exit
        || reference.exit_code != target.exit_code
    {
        return TargetOutcome {
            status: "failed".to_owned(),
            diagnostic: Some(format!(
                "exit-code mismatch: reference={} target={} target_stderr={}",
                reference.exit_code, target.exit_code, target_stderr_head
            )),
            reference_exit: reference.exit_code,
            target_exit: Some(target.exit_code),
            reference_stdout: truncate(&ref_norm, 1024),
            target_stdout: truncate(&tgt_norm, 1024),
            target_stderr_head,
            reference_elapsed_ns,
            target_elapsed_ns,
        };
    }

    // Both engines rejected the script, and printed the same thing doing it.
    // Whether that is agreement depends entirely on *why* the target
    // rejected, which only the corpus can say -- see
    // `BeyondCase::expected_target_stderr_contains`.
    let shared_rejection = if ref_norm == tgt_norm && reference.exit_code != 0 {
        Some(classify_shared_rejection(
            case,
            &target.stderr,
            target_bin,
            timeout,
        ))
    } else {
        None
    };

    if ref_norm == tgt_norm
        && (reference.exit_code == 0 || matches!(shared_rejection, Some(SharedRejection::Agrees)))
    {
        TargetOutcome {
            status: "passed".to_owned(),
            diagnostic: None,
            reference_exit: reference.exit_code,
            target_exit: Some(target.exit_code),
            reference_stdout: truncate(&ref_norm, 256),
            target_stdout: truncate(&tgt_norm, 256),
            target_stderr_head,
            reference_elapsed_ns,
            target_elapsed_ns,
        }
    } else {
        let diagnostic = if let Some(rejection) = shared_rejection {
            rejection.diagnostic(reference.exit_code, target.exit_code, &target_stderr_head)
        } else if ref_norm == tgt_norm {
            format!(
                "matching nonzero exit is not a semantic pass: reference={} target={} target_stderr={}",
                reference.exit_code, target.exit_code, target_stderr_head
            )
        } else {
            format!(
                "stdout mismatch: reference={:?} target={:?} target_stderr={}",
                truncate(&ref_norm, 256),
                truncate(&tgt_norm, 256),
                target_stderr_head
            )
        };
        TargetOutcome {
            status: "failed".to_owned(),
            diagnostic: Some(diagnostic),
            reference_exit: reference.exit_code,
            target_exit: Some(target.exit_code),
            reference_stdout: truncate(&ref_norm, 1024),
            target_stdout: truncate(&tgt_norm, 1024),
            target_stderr_head,
            reference_elapsed_ns,
            target_elapsed_ns,
        }
    }
}

/// Output of a target shell invocation. Mirrors `engine::PsqlOutput`.
struct TargetOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

fn invoke_target(target_bin: &Path, stdin: &str, timeout: Duration) -> Result<TargetOutput> {
    let mut command = Command::new(target_bin);
    command
        .arg("-batch")
        .arg("-bail")
        .arg("--error-exit")
        .arg("3")
        .arg(":memory:");
    command.env("REDLINEDB_RESULT_DIALECT", "postgres");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .with_context(|| format!("spawn target binary {}", target_bin.display()))?;
    {
        let mut child_stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("no stdin pipe for target child"))?;
        child_stdin
            .write_all(stdin.as_bytes())
            .context("write target stdin")?;
    }
    let started = Instant::now();
    let mut completed = false;
    loop {
        match child.try_wait()? {
            Some(_) => {
                completed = true;
                break;
            }
            None => {
                if started.elapsed() >= timeout {
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
    if !completed {
        let _ = child.kill();
        let _ = child.wait();
        bail!("target binary timed out after {:?}", timeout);
    }
    let out = child
        .wait_with_output()
        .context("target binary wait_with_output")?;
    Ok(TargetOutput {
        stdout: String::from_utf8_lossy(&out.stdout)
            .replace("\r\n", "\n")
            .replace('\r', "\n"),
        stderr: String::from_utf8_lossy(&out.stderr)
            .replace("\r\n", "\n")
            .replace('\r', "\n"),
        exit_code: out.status.code().unwrap_or(-1),
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_owned()
    } else {
        // Find a UTF-8 char boundary at or before `max` so we never slice
        // through a multi-byte sequence (case stdouts can contain Unicode
        // — e.g. the German eszett case 20049).
        let mut cut = max;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        let mut out = s[..cut].to_owned();
        out.push('…');
        out
    }
}

fn first_nonempty_line(s: &str) -> String {
    s.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .to_owned()
}

/// Why a case that both engines rejected did or did not count as agreement.
enum SharedRejection {
    /// The corpus declared the message, the target produced it, and the
    /// target got as far as the statement under test.
    Agrees,
    /// The corpus says nothing about what the target must say, so a shared
    /// non-zero exit proves only that both engines were unhappy.
    Undeclared,
    /// The corpus declared a message the target did not produce -- it
    /// rejected the script for some other reason.
    WrongReason { expected: String, actual: String },
    /// The target never reached the statement under test: its setup failed.
    /// Cases 20021 and 20023 land here, on `DROP TYPE` and `DROP DOMAIN`.
    SetupFailed { stderr: String },
}

impl SharedRejection {
    fn diagnostic(&self, reference_exit: i32, target_exit: i32, stderr_head: &str) -> String {
        match self {
            Self::Agrees => unreachable!("an agreeing rejection is a pass"),
            Self::Undeclared => format!(
                "matching nonzero exit is not a semantic pass: reference={reference_exit} \
                 target={target_exit} target_stderr={stderr_head}. Set \
                 expected_target_stderr_contains on this case if the target is \
                 expected to reject it, and for this reason",
            ),
            Self::WrongReason { expected, actual } => format!(
                "target rejected for a different reason: expected stderr to contain \
                 {expected:?}, got {actual:?}",
            ),
            Self::SetupFailed { stderr } => {
                format!("target failed during setup, before the statement under test: {stderr:?}",)
            }
        }
    }
}

/// A shared non-zero exit counts as agreement only when the corpus declares
/// the message the target must produce, the target produces it, and the
/// target executed the case's setup successfully -- so the rejection came
/// from the statement under test rather than from something before it.
fn classify_shared_rejection(
    case: &BeyondCase,
    target_stderr: &str,
    target_bin: &Path,
    timeout: Duration,
) -> SharedRejection {
    let Some(expected) = case.expected_target_stderr_contains.as_deref() else {
        return SharedRejection::Undeclared;
    };
    if !target_stderr.contains(expected) {
        return SharedRejection::WrongReason {
            expected: expected.to_owned(),
            actual: truncate(&first_nonempty_line(target_stderr), 256),
        };
    }
    // Re-run the setup alone against a fresh in-memory database. We do not
    // care what it produces, only whether the target can execute it at all:
    // if it cannot, the combined run died before the statement the case is
    // about, and the matching exit code means nothing.
    if let Some(setup) = &case.setup_stdin {
        let stdin = format!("{SQLITE_FORMATTING_PREAMBLE}{setup}");
        match invoke_target(target_bin, &stdin, timeout) {
            Ok(out) if out.exit_code == 0 => {}
            Ok(out) => {
                return SharedRejection::SetupFailed {
                    stderr: truncate(&first_nonempty_line(&out.stderr), 256),
                };
            }
            Err(err) => {
                return SharedRejection::SetupFailed {
                    stderr: format!("setup invocation error: {err}"),
                };
            }
        }
    }
    SharedRejection::Agrees
}

fn assembled_stdin(case: &BeyondCase) -> String {
    let mut s = String::new();
    if let Some(setup) = &case.setup_stdin {
        s.push_str(setup);
        if !setup.ends_with('\n') {
            s.push('\n');
        }
    }
    s.push_str(&case.stdin);
    if !case.stdin.ends_with('\n') {
        s.push('\n');
    }
    s
}

fn normalize_for_compare(text: &str, case: &BeyondCase) -> String {
    let normalized = apply_chain(text, &case.value_normalizers);
    match case.compare_mode {
        CompareMode::OrderedRows => normalized,
        CompareMode::SortedRows => {
            let mut lines: Vec<&str> = normalized.lines().collect();
            lines.sort();
            lines.join("\n") + if normalized.ends_with('\n') { "\n" } else { "" }
        }
        CompareMode::Set => {
            let mut lines: Vec<&str> = normalized.lines().collect();
            lines.sort();
            lines.dedup();
            lines.join("\n") + if normalized.ends_with('\n') { "\n" } else { "" }
        }
    }
}

// Allow oracle to write a JSONL fragment that the legacy emitter can
// concatenate into its output.
#[allow(dead_code)]
pub fn append_outcomes_jsonl(outcomes: &[CaseOutcome], output: &Path) -> Result<()> {
    let mut buf = String::new();
    for o in outcomes {
        let record = serde_json::json!({
            "suite": "beyond_sqlite",
            "case_id": format!("BEYOND-CASE-{:05}", o.case_id),
            "name": o.name,
            "feature_rank": o.feature_rank,
            "status": o.status,
            "diagnostic": o.diagnostic,
            "sample_role": "measured:1",
            "category": "beyond_sqlite_oracle",
        });
        buf.push_str(&serde_json::to_string(&record)?);
        buf.push('\n');
    }
    let mut existing = fs::read_to_string(output).unwrap_or_default();
    existing.push_str(&buf);
    fs::write(output, existing)
        .with_context(|| format!("append oracle outcomes to {}", output.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires REDLINE_TESTING_POSTGRES_URL or the documented local PostgreSQL socket"]
    fn postgres_self_compare_all_published_cases() {
        let (summary, outcomes) = run_cases().expect("run published beyond-SQLite cases");
        let blockers = outcomes
            .iter()
            .filter(|outcome| outcome.status != "passed")
            .take(20)
            .map(|outcome| {
                format!(
                    "{} {}: {}",
                    outcome.case_id,
                    outcome.status,
                    outcome.diagnostic.as_deref().unwrap_or("no diagnostic")
                )
            })
            .collect::<Vec<_>>();

        assert!(summary.total > 0, "published beyond-SQLite corpus is empty");
        assert_eq!(summary.failed, 0, "reference failures: {blockers:#?}");
        assert_eq!(
            summary.skipped_unavailable, 0,
            "PostgreSQL unavailable or invocation failed: {blockers:#?}"
        );
        assert_eq!(
            summary.skipped_feature_missing, 0,
            "published cases require unavailable features: {blockers:#?}"
        );
        assert_eq!(
            summary.passed, summary.total,
            "not every published case passed psql self-compare: {blockers:#?}"
        );
    }
}

#[cfg(test)]
mod completeness_tests {
    use super::*;
    #[test]
    fn disabled_and_duplicate_comparisons_are_rejected() {
        let cases: Vec<serde_json::Value> = serde_json::from_str(MANIFEST).unwrap();
        assert!(parse_cases(&serde_json::json!([cases[0], cases[0]]).to_string()).is_err());
        let mut disabled = cases[0].clone();
        disabled["requires_engine_features"] = serde_json::json!(["unimplemented"]);
        assert!(parse_cases(&serde_json::json!([disabled]).to_string()).is_err());
        assert!(parse_cases("[]").is_err());
    }
}
