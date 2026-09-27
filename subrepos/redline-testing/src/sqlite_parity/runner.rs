use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::case::Case;
use super::compare::{comparable, contract_text, describe, first_difference};
use super::engine::{EngineOutput, EngineSpec, SkippedCase};
use super::normalize::normalize_output;
use super::report;

/// What decided a sample's `status` (SQ-02).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictReason {
    /// The reference kept the case's declared contract and the target
    /// matched the reference.
    Passed,
    /// The case was not run.
    Skipped,
    /// The reference shell did not do what the case declares: wrong exit
    /// code, a missing declared fragment, different stdout, or death by a
    /// signal. Agreement with such a reference proves nothing.
    ReferenceContractFailure,
    /// The reference kept the case's contract and the target did not: a
    /// different exit code, a missing declared fragment (a "no such table"
    /// where the case declares a UNIQUE failure), or death by a signal.
    TargetSemanticFailure,
    /// The target kept the contract, but its exit code or output bytes
    /// differ from the reference's.
    DifferentialMismatch,
}

impl VerdictReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Skipped => "skipped",
            Self::ReferenceContractFailure => "reference_contract_failure",
            Self::TargetSemanticFailure => "target_semantic_failure",
            Self::DifferentialMismatch => "differential_mismatch",
        }
    }
}

/// The step of the verdict that decided it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictStage {
    /// Case selection, before anything ran (capability or rewrite skips).
    Selection,
    /// The reference output checked against the case's declared
    /// expectations, before any comparison.
    ReferenceContract,
    /// The target output checked against the same declared expectations.
    TargetContract,
    /// The target output compared with the reference output.
    Differential,
}

/// One sample's verdict: its reason, the deciding stage and, when it did not
/// pass, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub reason: VerdictReason,
    pub stage: VerdictStage,
    pub diagnostic: Option<String>,
}

impl Verdict {
    pub fn passed() -> Self {
        Self {
            reason: VerdictReason::Passed,
            stage: VerdictStage::Differential,
            diagnostic: None,
        }
    }

    pub fn failed(reason: VerdictReason, stage: VerdictStage, diagnostic: String) -> Self {
        Self {
            reason,
            stage,
            diagnostic: Some(diagnostic),
        }
    }

    pub fn is_pass(&self) -> bool {
        self.reason == VerdictReason::Passed
    }

    /// The record `status`: `passed`, `skipped` or `failed`.
    pub fn status(&self) -> &'static str {
        match self.reason {
            VerdictReason::Passed => "passed",
            VerdictReason::Skipped => "skipped",
            VerdictReason::ReferenceContractFailure
            | VerdictReason::TargetSemanticFailure
            | VerdictReason::DifferentialMismatch => "failed",
        }
    }
}

/// A case that failed, and every reason its failing samples gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseFailure {
    pub case_id: String,
    pub name: String,
    pub verdict_reasons: BTreeSet<VerdictReason>,
}

/// A run's case counts. Failures are counted and listed here, never raised:
/// the caller decides whether they are acceptable (`KnownFailures::gate`)
/// after the evidence is written.
#[derive(Debug, Clone, Default)]
pub struct RunSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub elapsed: Duration,
    pub slowest: Vec<(String, u128)>,
    /// Every failed case, in run order. `failures.len() == failed` for the
    /// SQLite-shell suites.
    pub failures: Vec<CaseFailure>,
    /// Every case skipped at selection, in run order.
    pub skipped_case_ids: Vec<String>,
}

impl RunSummary {
    /// Counts a case that was skipped at selection.
    pub fn record_skip(&mut self, case: &Case) {
        self.total += 1;
        self.skipped += 1;
        self.skipped_case_ids.push(case.display_id());
    }

    /// Counts a case that ran; `failure` is `Some` when a sample failed.
    pub fn record_run(&mut self, failure: Option<CaseFailure>) {
        self.total += 1;
        match failure {
            Some(failure) => {
                self.failed += 1;
                self.failures.push(failure);
            }
            None => self.passed += 1,
        }
    }
}

pub fn compare_cases(
    cases: &[Case],
    skipped: &[SkippedCase],
    reference: &EngineSpec,
    target: &EngineSpec,
    out: &Path,
    tmp_root: impl AsRef<Path>,
    workers: usize,
    warmup: usize,
    repetitions: usize,
    sqlite_version: Option<String>,
    progress: bool,
    memory_samples: bool,
) -> Result<RunSummary> {
    let tmp_root = tmp_root.as_ref();
    let started = Instant::now();
    let mut summary = RunSummary::default();
    for skipped_case in skipped {
        summary.record_skip(&skipped_case.case);
        let artifact = report::write_skip_artifact(&skipped_case.case, &skipped_case.reason)?;
        report::append_jsonl(
            Some(out),
            &report::skipped_compare_record(
                &skipped_case.case,
                &reference.name,
                &target.name,
                sqlite_version.clone(),
                Some(artifact),
                Some(skipped_case.reason.clone()),
            ),
        )?;
    }
    let total_samples = warmup.saturating_add(repetitions);
    let case_runs = run_case_set(
        cases,
        reference,
        target,
        tmp_root.to_path_buf(),
        workers,
        total_samples,
        warmup,
        sqlite_version,
        progress,
        memory_samples,
    )?;
    for case_run in case_runs {
        for record in &case_run.records {
            report::append_jsonl(Some(out), record)?;
        }
        summary.slowest.extend(case_run.slowest);
        summary.record_run(case_run.failure);
    }
    summary.elapsed = started.elapsed();
    Ok(finish_summary(summary, "sqlite_parity", progress))
}

struct CaseRun {
    records: Vec<report::CompareRecord>,
    failure: Option<CaseFailure>,
    slowest: Vec<(String, u128)>,
}

fn run_case_set(
    cases: &[Case],
    reference: &EngineSpec,
    target: &EngineSpec,
    tmp_root: PathBuf,
    workers: usize,
    total_samples: usize,
    warmup: usize,
    sqlite_version: Option<String>,
    progress: bool,
    memory_samples: bool,
) -> Result<Vec<CaseRun>> {
    if workers <= 1 || cases.len() <= 1 {
        return cases
            .iter()
            .map(|case| {
                run_one_case(
                    case,
                    reference,
                    target,
                    &tmp_root,
                    total_samples,
                    warmup,
                    sqlite_version.clone(),
                    progress,
                    memory_samples,
                )
            })
            .collect();
    }

    let workers = workers.min(cases.len());
    let cases = Arc::new(cases.to_vec());
    let next = Arc::new(AtomicUsize::new(0));
    let first_error = Arc::new(Mutex::new(None::<String>));
    let (tx, rx) = mpsc::channel::<(usize, Result<CaseRun>)>();
    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let cases = Arc::clone(&cases);
        let next = Arc::clone(&next);
        let first_error = Arc::clone(&first_error);
        let tx = tx.clone();
        let reference = reference.clone();
        let target = target.clone();
        let tmp_root = tmp_root.clone();
        let sqlite_version = sqlite_version.clone();
        handles.push(std::thread::spawn(move || {
            loop {
                if first_error.lock().is_ok_and(|guard| guard.is_some()) {
                    break;
                }
                let index = next.fetch_add(1, Ordering::SeqCst);
                let Some(case) = cases.get(index) else {
                    break;
                };
                let result = run_one_case(
                    case,
                    &reference,
                    &target,
                    &tmp_root,
                    total_samples,
                    warmup,
                    sqlite_version.clone(),
                    progress,
                    memory_samples,
                );
                if let Err(err) = &result
                    && let Ok(mut guard) = first_error.lock()
                {
                    *guard = Some(err.to_string());
                }
                if tx.send((index, result)).is_err() {
                    break;
                }
            }
        }));
    }
    drop(tx);

    let mut ordered = (0..cases.len()).map(|_| None).collect::<Vec<_>>();
    for (index, result) in rx {
        ordered[index] = Some(result);
    }
    for handle in handles {
        handle
            .join()
            .map_err(|_| anyhow::anyhow!("sqlite parity worker thread panicked"))?;
    }
    ordered
        .into_iter()
        .enumerate()
        .map(|(index, result)| {
            result.unwrap_or_else(|| {
                Err(anyhow::anyhow!("sqlite parity worker skipped case {index}"))
            })
        })
        .collect()
}

fn run_one_case(
    case: &Case,
    reference: &EngineSpec,
    target: &EngineSpec,
    tmp_root: &Path,
    total_samples: usize,
    warmup: usize,
    sqlite_version: Option<String>,
    progress: bool,
    memory_samples: bool,
) -> Result<CaseRun> {
    if progress {
        eprintln!("sqlite_parity case={} status=running", case.display_id());
    }
    let mut failure_reasons = BTreeSet::new();
    let mut records = Vec::with_capacity(total_samples);
    let mut slowest = Vec::new();
    for sample_index in 0..total_samples {
        let measured_index = sample_index.checked_sub(warmup);
        let sample_role = if let Some(index) = measured_index {
            format!("measured:{}", index.saturating_add(1))
        } else {
            "warmup".to_owned()
        };
        let reference_output = reference.run_case(case, tmp_root, memory_samples)?;
        let target_output = target.run_case(case, tmp_root, memory_samples)?;
        let verdict = judge_sample(case, &reference_output, &target_output);
        let artifact = if let Some(reason) = &verdict.diagnostic {
            let artifact =
                report::write_failure_artifact(case, &[&reference_output, &target_output], reason)?;
            eprintln!(
                "sqlite_parity failure case={} verdict={:?} reason={} artifact={}",
                case.display_id(),
                verdict.reason,
                reason,
                artifact.display()
            );
            Some(artifact)
        } else {
            None
        };
        records.push(report::compare_record(
            case,
            &reference_output,
            &target_output,
            sample_index,
            measured_index.map(|index| index.saturating_add(1)),
            sample_role,
            sqlite_version.clone(),
            &verdict,
            artifact,
        ));
        if measured_index.is_some() {
            slowest.push((case.display_id(), target_output.elapsed.as_nanos()));
        }
        if !verdict.is_pass() {
            failure_reasons.insert(verdict.reason);
        }
    }
    let failure = case_failure(case, failure_reasons);
    if progress {
        let status = if failure.is_some() {
            "failed"
        } else {
            "passed"
        };
        eprintln!("sqlite_parity case={} status={status}", case.display_id());
    }
    Ok(CaseRun {
        records,
        failure,
        slowest,
    })
}

/// The case's failure, if any sample failed.
pub(super) fn case_failure(
    case: &Case,
    verdict_reasons: BTreeSet<VerdictReason>,
) -> Option<CaseFailure> {
    (!verdict_reasons.is_empty()).then(|| CaseFailure {
        case_id: case.display_id(),
        name: case.name.clone(),
        verdict_reasons,
    })
}

/// One sample's verdict. The reference must first keep the contract the
/// case declares; only then does anything the target does mean something.
/// The target must keep the same contract (a UNIQUE failure is not a "no
/// such table"), and then print what the reference printed, byte for byte.
pub(super) fn judge_sample(
    case: &Case,
    reference: &EngineOutput,
    target: &EngineOutput,
) -> Verdict {
    if let Err(reason) = validate_reference_contract(case, reference) {
        return Verdict::failed(
            VerdictReason::ReferenceContractFailure,
            VerdictStage::ReferenceContract,
            format!("reference contract: {reason:#}"),
        );
    }
    if let Err(reason) = validate_expected(case, target) {
        return Verdict::failed(
            VerdictReason::TargetSemanticFailure,
            VerdictStage::TargetContract,
            format!("target contract: {reason:#}"),
        );
    }
    match validate_compare(case, reference, target) {
        Ok(()) => Verdict::passed(),
        Err(reason) => Verdict::failed(
            VerdictReason::DifferentialMismatch,
            VerdictStage::Differential,
            format!("{reason:#}"),
        ),
    }
}

/// The expectations a case declares about one engine's run: it exited
/// normally with `expected_exit` (a signal is never a pass), and its stdout,
/// stderr and stdout+stderr contain every declared fragment. Fragments are
/// diagnostics: they are matched on `contract_text`, whatever the case's
/// `comparison_mode`, and hold whether or not the case compares stdout.
pub(super) fn validate_expected(case: &Case, output: &EngineOutput) -> Result<()> {
    let stdout = contract_text(&output.stdout);
    let stderr = contract_text(&output.stderr);
    let Some(code) = output.status_code else {
        bail!(
            "{} was terminated by a signal; case expects exit {}; stderr `{}`",
            output.engine,
            case.expected_exit,
            normalize_output(&stderr)
        );
    };
    if code != case.expected_exit {
        bail!(
            "{} exited {code}; case expects exit {}; stderr `{}`",
            output.engine,
            case.expected_exit,
            normalize_output(&stderr)
        );
    }
    let combined = format!("{stdout}{stderr}");
    for (stream, text, needles) in [
        ("stdout", &stdout, &case.expected_stdout_contains),
        ("stderr", &stderr, &case.expected_stderr_contains),
        ("stdout+stderr", &combined, &case.expected_combined_contains),
    ] {
        if let Some(needle) = needles
            .iter()
            .find(|needle| !text.contains(needle.as_str()))
        {
            bail!(
                "{} {stream} lacks declared fragment {needle:?}; got `{}`",
                output.engine,
                normalize_output(text)
            );
        }
    }
    Ok(())
}

/// The reference's contract: `validate_expected`, and when the case compares
/// stdout and declares it, exactly that stdout after normalization (the
/// corpus stores `expected_stdout` as normalized text).
fn validate_reference_contract(case: &Case, reference: &EngineOutput) -> Result<()> {
    validate_expected(case, reference)?;
    if case.compare_stdout
        && let Some(expected) = &case.expected_stdout
    {
        let expected = normalize_output(expected);
        let actual = normalize_output(&contract_text(&comparable(
            case,
            &reference.engine,
            &reference.stdout,
        )));
        if actual != expected {
            bail!(
                "{} stdout differs from expected_stdout: expected `{expected}`, got `{actual}`",
                reference.engine
            );
        }
    }
    Ok(())
}

/// The differential: the same exit, and the same bytes on stdout (when the
/// case compares it) and on stderr (when the reference succeeded; a failing
/// run's error text is held to the declared fragments instead, since the
/// two shells word the same error differently).
fn validate_compare(case: &Case, reference: &EngineOutput, target: &EngineOutput) -> Result<()> {
    for output in [reference, target] {
        if output.status_code.is_none() {
            bail!("{} was terminated by a signal", output.engine);
        }
    }
    if reference.status_code != target.status_code {
        bail!(
            "exit mismatch: reference {:?}, target {:?}",
            reference.status_code,
            target.status_code
        );
    }
    if !case.compare_stdout {
        return Ok(());
    }
    compare_stream(case, "stdout", reference, target, |output| &output.stdout)?;
    if reference.status_code != Some(0) || case.status == "catalog_only" {
        return Ok(());
    }
    compare_stream(case, "stderr", reference, target, |output| &output.stderr)
}

fn compare_stream(
    case: &Case,
    stream: &str,
    reference: &EngineOutput,
    target: &EngineOutput,
    bytes: impl Fn(&EngineOutput) -> &[u8],
) -> Result<()> {
    let reference_bytes = comparable(case, &reference.engine, bytes(reference));
    let target_bytes = comparable(case, &target.engine, bytes(target));
    if reference_bytes != target_bytes {
        bail!(
            "{stream} mismatch at byte {}: reference {} ({} bytes), target {} ({} bytes)",
            first_difference(&reference_bytes, &target_bytes),
            describe(&reference_bytes),
            reference_bytes.len(),
            describe(&target_bytes),
            target_bytes.len()
        );
    }
    Ok(())
}

pub fn validate_compare_engines(reference: &EngineSpec, target: &EngineSpec) -> Result<()> {
    let reference_identity = reference.binary_identity()?;
    let target_identity = target.binary_identity()?;
    if reference_identity.executable_path == target_identity.executable_path
        || reference_identity.executable_sha256 == target_identity.executable_sha256
    {
        bail!(
            "sqlite parity compare requires distinct reference and target binaries: reference={} target={}",
            reference_identity.executable_path,
            target_identity.executable_path
        );
    }
    if target.name.eq_ignore_ascii_case("redlinedb")
        && !target_identity
            .version
            .to_ascii_lowercase()
            .contains("redlinedb")
    {
        bail!(
            "sqlite parity target `{}` must identify as RedlineDB via --version, got `{}` from {}",
            target.name,
            target_identity.version,
            target_identity.executable_path
        );
    }
    Ok(())
}

/// Sorts the slowest cases and prints the counts. Failures are left in the
/// summary for the caller's known-failures gate; they are never an error
/// here, so the run's records and evidence are always written.
pub(super) fn finish_summary(mut summary: RunSummary, suite: &str, progress: bool) -> RunSummary {
    summary
        .slowest
        .sort_by_key(|slowest| std::cmp::Reverse(slowest.1));
    summary.slowest.truncate(10);
    if progress {
        eprintln!(
            "{suite} total={} passed={} failed={} skipped={} elapsed_ns={}",
            summary.total,
            summary.passed,
            summary.failed,
            summary.skipped,
            summary.elapsed.as_nanos()
        );
        eprintln!("{suite} slowest={:?}", summary.slowest);
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::{Verdict, VerdictReason, VerdictStage, judge_sample, validate_compare};
    use crate::sqlite_parity::test_fixtures::{output, unique_case};

    #[test]
    fn reference_contract_violation_fails_even_when_engines_agree() {
        // Both engines exit 1 with the same wrong error: the reference does
        // not print the UNIQUE failure the case declares, so the agreement
        // proves nothing.
        let case = unique_case();
        let wrong_error = "Parse error near line 3: no such table: t\n";
        let verdict = judge_sample(
            &case,
            &output("sqlite3", Some(1), "", wrong_error),
            &output("redlinedb", Some(1), "", wrong_error),
        );
        assert_eq!(verdict.reason, VerdictReason::ReferenceContractFailure);
        assert_eq!(verdict.stage, VerdictStage::ReferenceContract);
        assert_eq!(verdict.status(), "failed");
        let diagnostic = verdict.diagnostic.expect("failure diagnostic");
        assert!(
            diagnostic.contains("UNIQUE constraint failed: t.x"),
            "{diagnostic}"
        );

        // Both engines succeed where the case declares a failure.
        let verdict = judge_sample(
            &case,
            &output("sqlite3", Some(0), "1\n", ""),
            &output("redlinedb", Some(0), "1\n", ""),
        );
        assert_eq!(verdict.reason, VerdictReason::ReferenceContractFailure);

        // Both engines print the declared stdout fragment only on stderr.
        let mut stdout_case = unique_case();
        stdout_case.expected_exit = 0;
        stdout_case.expected_stderr_contains.clear();
        stdout_case.expected_stdout_contains = vec!["R163".to_owned()];
        let verdict = judge_sample(
            &stdout_case,
            &output("sqlite3", Some(0), "", "R163\n"),
            &output("redlinedb", Some(0), "", "R163\n"),
        );
        assert_eq!(verdict.reason, VerdictReason::ReferenceContractFailure);

        // Both engines agree on stdout that is not the declared stdout.
        let mut exact_case = stdout_case.clone();
        exact_case.expected_stdout_contains.clear();
        exact_case.expected_stdout = Some("a   b\n".to_owned());
        let verdict = judge_sample(
            &exact_case,
            &output("sqlite3", Some(0), "a    b\n", ""),
            &output("redlinedb", Some(0), "a    b\n", ""),
        );
        assert_eq!(verdict.reason, VerdictReason::ReferenceContractFailure);

        // The declared contract holds on the reference and the target agrees.
        let right_error = "Runtime error near line 3: UNIQUE constraint failed: t.x (19)\n";
        let verdict = judge_sample(
            &case,
            &output("sqlite3", Some(1), "", right_error),
            &output("redlinedb", Some(1), "", right_error),
        );
        assert_eq!(verdict, Verdict::passed());
    }

    #[test]
    fn reference_contract_checks_exact_stdout_after_normalization() {
        let mut case = unique_case();
        case.expected_exit = 0;
        case.expected_stderr_contains.clear();
        case.expected_stdout = Some("1\n2\n".to_owned());
        // The reference keeps the declared stdout once normalized ...
        let verdict = judge_sample(
            &case,
            &output("sqlite3", Some(0), "1\r\n2\r\n  \n", ""),
            &output("redlinedb", Some(0), "1\r\n2\r\n  \n", ""),
        );
        assert_eq!(verdict, Verdict::passed());
        // ... but that normalization is the declared contract's alone: the
        // differential still compares the bytes each shell printed.
        let verdict = judge_sample(
            &case,
            &output("sqlite3", Some(0), "1\r\n2\r\n  \n", ""),
            &output("redlinedb", Some(0), "1\n2\n", ""),
        );
        assert_eq!(verdict.reason, VerdictReason::DifferentialMismatch);
        // compare_stdout=false leaves stdout to the fragments.
        case.compare_stdout = false;
        let verdict = judge_sample(
            &case,
            &output("sqlite3", Some(0), "other\n", ""),
            &output("redlinedb", Some(0), "different\n", ""),
        );
        assert_eq!(verdict, Verdict::passed());
    }

    #[test]
    fn signal_terminated_children_never_pass() {
        let mut case = unique_case();
        case.expected_exit = 0;
        case.expected_stderr_contains.clear();
        let verdict = judge_sample(
            &case,
            &output("sqlite3", None, "", ""),
            &output("redlinedb", None, "", ""),
        );
        assert_eq!(verdict.reason, VerdictReason::ReferenceContractFailure);
        assert!(
            verdict
                .diagnostic
                .as_deref()
                .is_some_and(|diagnostic| diagnostic.contains("terminated by a signal")),
            "{verdict:?}"
        );
        // A healthy reference does not rescue a target killed by a signal:
        // the target breaks the case's contract before any comparison.
        let verdict = judge_sample(
            &case,
            &output("sqlite3", Some(0), "", ""),
            &output("redlinedb", None, "", ""),
        );
        assert_eq!(verdict.reason, VerdictReason::TargetSemanticFailure);
        assert_eq!(verdict.stage, VerdictStage::TargetContract);
        assert!(
            verdict.diagnostic.as_deref().is_some_and(
                |diagnostic| diagnostic.contains("redlinedb was terminated by a signal")
            ),
            "{verdict:?}"
        );
        // Nor can the differential alone pass two signal deaths.
        assert!(
            validate_compare(
                &case,
                &output("sqlite3", None, "", ""),
                &output("redlinedb", None, "", "")
            )
            .is_err()
        );
    }
}
