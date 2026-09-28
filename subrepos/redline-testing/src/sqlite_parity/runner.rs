use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::case::Case;
use super::compare::{comparable, contract_text, describe, first_difference};
use super::engine::{EngineOutput, EngineSpec, RejectedCase, SkippedCase};
use super::normalize::normalize_output;
use super::record_sink::RecordSink;
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
    /// An engine run left no whole result to judge: it timed out, passed
    /// the output cap or could not be started (SQ-09). Never a baseline
    /// entry: a known failure must fail the same way every run.
    ExecutionFailure,
    /// The reference shell lacks a capability the case declares, so it
    /// cannot stand for SQLite on this case (SQ-05). Never skipped.
    ReferenceCapabilityMissing,
    /// The target cannot run the case (a missing capability, or an RQL
    /// rewrite that cannot express it) and the scope policy lists no
    /// exception for it (SQ-05).
    TargetUnsupported,
}

impl VerdictReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Skipped => "skipped",
            Self::ReferenceContractFailure => "reference_contract_failure",
            Self::TargetSemanticFailure => "target_semantic_failure",
            Self::DifferentialMismatch => "differential_mismatch",
            Self::ExecutionFailure => "execution_failure",
            Self::ReferenceCapabilityMissing => "reference_capability_missing",
            Self::TargetUnsupported => "target_unsupported",
        }
    }
}

/// The step of the verdict that decided it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictStage {
    /// Case selection, before anything ran (capability or rewrite skips).
    Selection,
    /// An engine run ended without a whole result (SQ-09).
    Execution,
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
            | VerdictReason::DifferentialMismatch
            | VerdictReason::ExecutionFailure
            | VerdictReason::ReferenceCapabilityMissing
            | VerdictReason::TargetUnsupported => "failed",
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
    /// Every case whose samples all passed, in completion order.
    pub passed_case_ids: Vec<String>,
}

impl RunSummary {
    /// Counts a case that was skipped at selection.
    pub fn record_skip(&mut self, case: &Case) {
        self.total += 1;
        self.skipped += 1;
        self.skipped_case_ids.push(case.display_id());
    }

    /// Counts a case that ran; `failure` is `Some` when a sample failed.
    pub fn record_run(&mut self, case_id: String, failure: Option<CaseFailure>) {
        self.total += 1;
        match failure {
            Some(failure) => {
                self.failed += 1;
                self.failures.push(failure);
            }
            None => {
                self.passed += 1;
                self.passed_case_ids.push(case_id);
            }
        }
    }

    /// Counts a case that failed at selection, before anything ran.
    pub fn record_selection_failure(&mut self, case: &Case, reason: VerdictReason) {
        self.record_run(
            case.display_id(),
            case_failure(case, BTreeSet::from([reason])),
        );
    }
}

/// One case as each engine runs it: the same case for `sqlite_parity`
/// and `memory`, the RQL rewrite on the target side for `rql_phase1`. The
/// reference case is the one judged and recorded.
#[derive(Debug, Clone)]
pub(super) struct CasePair {
    pub reference: Case,
    pub target: Case,
}

impl CasePair {
    pub fn same(case: Case) -> Self {
        Self {
            target: case.clone(),
            reference: case,
        }
    }
}

/// What every case of one suite run shares.
pub(super) struct SuiteRun<'a> {
    /// The label of progress and failure lines.
    pub label: &'static str,
    pub reference: &'a EngineSpec,
    pub target: &'a EngineSpec,
    pub tmp_root: &'a Path,
    pub warmup: usize,
    pub repetitions: usize,
    pub sqlite_version: Option<String>,
    pub progress: bool,
    pub memory_samples: bool,
}

/// Runs a suite's cases on `workers` threads and streams each case's
/// records to `sink` as it completes (SQ-09). Cases the scope policy skips
/// and cases rejected at selection (SQ-05) are written first, each as one
/// placeholder record. An engine that times out, floods, crashes or cannot
/// start fails its case, not the run. Only a harness error (an artifact or
/// record that cannot be written) stops the run; the records of every case
/// finished by then stay on disk, and no completion marker is written.
pub(super) fn compare_cases(
    run: &SuiteRun<'_>,
    pairs: &[CasePair],
    skipped: &[SkippedCase],
    rejected: &[RejectedCase],
    workers: usize,
    sink: RecordSink,
) -> Result<RunSummary> {
    let started = Instant::now();
    let mut summary = RunSummary::default();
    for skipped_case in skipped {
        summary.record_skip(&skipped_case.case);
        let artifact = report::write_skip_artifact(&skipped_case.case, &skipped_case.reason)?;
        sink.write_case(&[report::skipped_compare_record(
            &skipped_case.case,
            &run.reference.name,
            &run.target.name,
            run.sqlite_version.clone(),
            Some(artifact),
            Some(skipped_case.reason.clone()),
            &skipped_case.policy_exception_id,
        )])?;
    }
    for rejected_case in rejected {
        summary.record_selection_failure(&rejected_case.case, rejected_case.verdict_reason);
        let artifact = report::write_skip_artifact(&rejected_case.case, &rejected_case.reason)?;
        eprintln!(
            "{} failure case={} verdict={:?} reason={} artifact={}",
            run.label,
            rejected_case.case.display_id(),
            rejected_case.verdict_reason,
            rejected_case.reason,
            artifact.display()
        );
        sink.write_case(&[report::selection_failure_record(
            &rejected_case.case,
            &run.reference.name,
            &run.target.name,
            run.sqlite_version.clone(),
            artifact,
            &Verdict::failed(
                rejected_case.verdict_reason,
                VerdictStage::Selection,
                rejected_case.reason.clone(),
            ),
        )])?;
    }
    run_pairs(run, pairs, workers, &sink, &mut summary)?;
    sink.finish()?;
    summary.elapsed = started.elapsed();
    Ok(finish_summary(summary, run.label, run.progress))
}

struct CaseRun {
    records: Vec<report::CompareRecord>,
    failure: Option<CaseFailure>,
    slowest: Vec<(String, u128)>,
}

/// What the collector keeps of a case whose records are already written.
struct CaseDone {
    case_id: String,
    failure: Option<CaseFailure>,
    slowest: Vec<(String, u128)>,
}

fn run_pairs(
    run: &SuiteRun<'_>,
    pairs: &[CasePair],
    workers: usize,
    sink: &RecordSink,
    summary: &mut RunSummary,
) -> Result<()> {
    let workers = workers.clamp(1, pairs.len().max(1));
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let (sender, receiver) = mpsc::channel::<Result<CaseDone>>();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let sender = sender.clone();
            let (next, stop) = (&next, &stop);
            scope.spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let Some(pair) = pairs.get(next.fetch_add(1, Ordering::SeqCst)) else {
                        break;
                    };
                    let done = run_one_case(run, pair).and_then(|case_run| {
                        sink.write_case(&case_run.records)?;
                        Ok(CaseDone {
                            case_id: pair.reference.display_id(),
                            failure: case_run.failure,
                            slowest: case_run.slowest,
                        })
                    });
                    if done.is_err() {
                        stop.store(true, Ordering::SeqCst);
                    }
                    if sender.send(done).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        let mut first_error = None;
        for done in receiver {
            match done {
                Ok(done) => {
                    summary.slowest.extend(done.slowest);
                    summary.record_run(done.case_id, done.failure);
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    })
}

fn run_one_case(run: &SuiteRun<'_>, pair: &CasePair) -> Result<CaseRun> {
    let case = &pair.reference;
    if run.progress {
        eprintln!("{} case={} status=running", run.label, case.display_id());
    }
    let total_samples = run.warmup.saturating_add(run.repetitions);
    let mut failure_reasons = BTreeSet::new();
    let mut records = Vec::with_capacity(total_samples);
    let mut slowest = Vec::new();
    for sample_index in 0..total_samples {
        let measured_index = sample_index.checked_sub(run.warmup);
        let sample_role = if let Some(index) = measured_index {
            format!("measured:{}", index.saturating_add(1))
        } else {
            "warmup".to_owned()
        };
        let reference_output =
            run.reference
                .run_case_bounded(&pair.reference, run.tmp_root, run.memory_samples);
        let target_output =
            run.target
                .run_case_bounded(&pair.target, run.tmp_root, run.memory_samples);
        let verdict = judge_sample(case, &reference_output, &target_output);
        let artifact = if let Some(reason) = &verdict.diagnostic {
            let artifact =
                report::write_failure_artifact(case, &[&reference_output, &target_output], reason)?;
            eprintln!(
                "{} failure case={} verdict={:?} reason={} artifact={}",
                run.label,
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
            run.sqlite_version.clone(),
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
    if run.progress {
        let status = if failure.is_some() {
            "failed"
        } else {
            "passed"
        };
        eprintln!("{} case={} status={status}", run.label, case.display_id());
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

/// One sample's verdict. Both engine runs must first have ended with a
/// whole result: a run killed at the deadline or the output cap, or one
/// that never started, has nothing to judge (SQ-09). The reference must
/// then keep the contract the case declares; only then does anything the
/// target does mean something. The target must keep the same contract (a
/// UNIQUE failure is not a "no such table"), and then print what the
/// reference printed, byte for byte.
pub(super) fn judge_sample(
    case: &Case,
    reference: &EngineOutput,
    target: &EngineOutput,
) -> Verdict {
    for output in [reference, target] {
        if !output.outcome.is_complete() {
            return Verdict::failed(
                VerdictReason::ExecutionFailure,
                VerdictStage::Execution,
                format!(
                    "{} {}",
                    output.engine,
                    output.failure.as_deref().unwrap_or(output.outcome.as_str())
                ),
            );
        }
    }
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
    use crate::sqlite_parity::bounded::ExecutionOutcome;
    use crate::sqlite_parity::test_fixtures::{incomplete, output, plain_case, unique_case};

    #[test]
    fn incomplete_runs_fail_before_any_contract() {
        // A run killed at the deadline or the output cap, or one that never
        // started, left no whole result: neither the contract nor the
        // differential may judge it, on either side.
        let case = plain_case();
        let healthy = |engine| output(engine, Some(0), "", "");
        for (outcome, failure) in [
            (ExecutionOutcome::Timeout, "timed out after 500 ms"),
            (ExecutionOutcome::OutputLimit, "wrote more than 64 bytes"),
            (ExecutionOutcome::SpawnError, "could not run: spawn failed"),
        ] {
            for (reference, target, engine) in [
                (
                    healthy("sqlite3"),
                    incomplete("redlinedb", outcome, failure),
                    "redlinedb",
                ),
                (
                    incomplete("sqlite3", outcome, failure),
                    healthy("redlinedb"),
                    "sqlite3",
                ),
            ] {
                let verdict = judge_sample(&case, &reference, &target);
                assert_eq!(
                    verdict.reason,
                    VerdictReason::ExecutionFailure,
                    "{verdict:?}"
                );
                assert_eq!(verdict.stage, VerdictStage::Execution);
                assert_eq!(verdict.status(), "failed");
                assert_eq!(
                    verdict.diagnostic.as_deref(),
                    Some(format!("{engine} {failure}").as_str())
                );
            }
        }
    }

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
