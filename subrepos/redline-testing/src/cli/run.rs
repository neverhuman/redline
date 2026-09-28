use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};

use crate::beyond_sqlite;
use crate::evidence::{self, EvidenceConfig, OfficialEvidenceConfig, OfficialSuiteEvidence};
use crate::sqlite_parity::{self, KnownFailures, ScopePolicy};

use super::args::{ProgressMode, RunArgs, Suite};

pub(crate) fn run_suite(args: RunArgs) -> Result<()> {
    let workers = resolve_workers(&args.workers)?;
    validate_samples(args.repetitions, args.warmup)?;
    // Every engine run is bounded (SQ-09); refuse to start if it cannot be.
    sqlite_parity::Limits::new(args.case_timeout_ms, args.max_output_bytes)?;
    sqlite_parity::check_kill()?;
    let tmp_root = resolve_tmp_root(&args.tmp_root)?;
    fs::create_dir_all(&tmp_root)
        .with_context(|| format!("create tmp root {}", tmp_root.display()))?;
    let sqlite_bin = resolve_sqlite_bin(&args.sqlite_bin);
    let known_failures = load_known_failures(args.sqlite_known_failures.as_deref())?;
    if !args.case_ids.is_empty() && matches!(args.suite, Suite::All | Suite::BeyondSqlite) {
        bail!(
            "--case-id narrows one SQLite-shell suite (sqlite_parity, memory or rql_phase1), not --suite {}",
            args.suite.as_str()
        );
    }
    let scope_policy = load_scope_policy(&args)?;

    match args.suite {
        Suite::All => run_all_suites(
            &args,
            workers,
            tmp_root,
            sqlite_bin,
            &known_failures,
            &scope_policy,
        ),
        Suite::SqliteParity | Suite::Memory | Suite::RqlPhase1 => {
            prepare_output(&args.output)?;
            let started = Instant::now();
            let summary = run_sqlite_like_suite(
                &args,
                args.suite,
                args.output.clone(),
                workers,
                tmp_root,
                sqlite_bin,
            )?;
            if progress_enabled(args.progress) {
                eprintln!(
                    "redline-testing {} total={} passed={} failed={} skipped={} elapsed_ns={}",
                    args.suite.as_str(),
                    summary.total,
                    summary.passed,
                    summary.failed,
                    summary.skipped,
                    started.elapsed().as_nanos()
                );
            }
            // The raw records and the suite's evidence are written; now
            // the failures must be exactly the known ones and the skips
            // exactly the listed ones.
            let known = known_failures.gate(args.suite.as_str(), &summary);
            let scope = scope_policy.gate(args.suite.as_str(), &summary);
            join_problems([known, scope])
        }
        Suite::BeyondSqlite => {
            prepare_output(&args.output)?;
            let summary = run_beyond_sqlite_suite(&args, args.output.clone())?;
            if progress_enabled(args.progress) {
                eprintln!(
                    "redline-testing beyond_sqlite total={} passed={} failed={} skipped={}",
                    summary.total, summary.passed, summary.failed, summary.skipped
                );
            }
            beyond_sqlite::gate::check(
                &args.output,
                args.postgres_regression_baseline.as_deref(),
                args.postgres_readme.as_deref(),
                &args.publication.policy(),
            )
        }
    }
}

/// The scope policy compiled into this runner (SQ-05), after the checks an
/// official run makes: `--suite all` with a known-failures baseline over
/// the whole corpus, and no expired exception.
fn load_scope_policy(args: &RunArgs) -> Result<ScopePolicy> {
    let policy = ScopePolicy::compiled()?;
    policy.check_corpus(&sqlite_parity::all_cases()?)?;
    if !args.official {
        return Ok(policy);
    }
    if !matches!(args.suite, Suite::All) {
        bail!(
            "--official runs --suite all, not --suite {}",
            args.suite.as_str()
        );
    }
    if !args.case_ids.is_empty() {
        bail!("--official runs the whole corpus; drop --case-id");
    }
    if args.sqlite_known_failures.is_none() {
        bail!(
            "--official needs --sqlite-known-failures: an official run publishes only known failures"
        );
    }
    // select_cases refuses it too; failing here writes nothing first.
    sqlite_parity::check_official_selection()?;
    policy.check_expiry(&sqlite_parity::today_utc())?;
    Ok(policy)
}

fn join_problems(results: impl IntoIterator<Item = Result<()>>) -> Result<()> {
    let problems = results
        .into_iter()
        .filter_map(Result::err)
        .map(|error| format!("{error:#}"))
        .collect::<Vec<_>>();
    if problems.is_empty() {
        Ok(())
    } else {
        bail!("{}", problems.join("\n"))
    }
}

/// The known-failures baseline, checked against the compiled-in corpus, or
/// none when the run names no baseline.
fn load_known_failures(path: Option<&Path>) -> Result<KnownFailures> {
    let Some(path) = path else {
        return Ok(KnownFailures::none());
    };
    let known_failures = KnownFailures::load(path)?;
    known_failures
        .check_corpus(&sqlite_parity::all_cases()?)
        .with_context(|| format!("known-failures baseline {}", path.display()))?;
    Ok(known_failures)
}

fn run_all_suites(
    args: &RunArgs,
    workers: usize,
    tmp_root: PathBuf,
    sqlite_bin: PathBuf,
    known_failures: &KnownFailures,
    scope_policy: &ScopePolicy,
) -> Result<()> {
    let generated_at_unix_ms = evidence::now_unix_ms();
    let output_dir = args
        .output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(output_dir).with_context(|| format!("create {}", output_dir.display()))?;
    let sqlite_output = output_dir.join("sqlite_parity.raw.jsonl");
    let memory_output = output_dir.join("memory.raw.jsonl");
    let rql_output = output_dir.join("rql_phase1.raw.jsonl");
    let beyond_output = output_dir.join("beyond_sqlite.raw.jsonl");

    prepare_output(&sqlite_output)?;
    let sqlite_summary = run_sqlite_like_suite(
        args,
        Suite::SqliteParity,
        sqlite_output.clone(),
        workers,
        tmp_root.clone(),
        sqlite_bin.clone(),
    )?;
    prepare_output(&memory_output)?;
    let memory_summary = run_sqlite_like_suite(
        args,
        Suite::Memory,
        memory_output.clone(),
        workers,
        tmp_root.clone(),
        sqlite_bin.clone(),
    )?;
    prepare_output(&rql_output)?;
    let rql_summary = run_sqlite_like_suite(
        args,
        Suite::RqlPhase1,
        rql_output.clone(),
        workers,
        tmp_root.clone(),
        sqlite_bin.clone(),
    )?;
    prepare_output(&beyond_output)?;
    let beyond_summary = run_beyond_sqlite_suite(args, beyond_output.clone())?;

    let mut combined = String::new();
    for path in [&sqlite_output, &memory_output, &rql_output, &beyond_output] {
        combined.push_str(
            &fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?,
        );
    }
    fs::write(&args.output, combined)
        .with_context(|| format!("write {}", args.output.display()))?;
    let all_manifest = write_all_manifest(
        output_dir,
        &args.output,
        [
            (
                "sqlite_parity",
                sqlite_output.as_path(),
                sqlite_summary.clone(),
            ),
            ("memory", memory_output.as_path(), memory_summary.clone()),
            ("rql_phase1", rql_output.as_path(), rql_summary.clone()),
            (
                "beyond_sqlite",
                beyond_output.as_path(),
                beyond_summary.clone(),
            ),
        ],
    )?;
    let postgres_gate = beyond_sqlite::gate::check(
        &beyond_output,
        args.postgres_regression_baseline.as_deref(),
        args.postgres_readme.as_deref(),
        &args.publication.policy(),
    );
    // Every suite's failures are judged against the baseline, but only
    // after every suite ran and the official evidence is written, so a
    // failing run still leaves complete evidence behind.
    let sqlite_gates = [
        ("sqlite_parity", &sqlite_summary),
        ("memory", &memory_summary),
        ("rql_phase1", &rql_summary),
    ]
    .map(|(suite, summary)| {
        join_problems([
            known_failures.gate(suite, summary),
            scope_policy.gate(suite, summary),
        ])
    });
    evidence::write_official_evidence(OfficialEvidenceConfig {
        output_dir: output_dir.to_path_buf(),
        all_output: args.output.clone(),
        all_manifest,
        target_bin: args.target_bin.clone(),
        sqlite_bin,
        tmp_root,
        workers: workers.to_string(),
        repetitions: args.repetitions,
        warmup: args.warmup,
        memory_samples: args.memory_samples,
        command_line: std::env::args().collect::<Vec<_>>(),
        generated_at_unix_ms,
        suites: vec![
            OfficialSuiteEvidence::new(
                "sqlite_parity",
                sqlite_output,
                evidence::suite_artifact_path(output_dir, "sqlite_parity", "summary.json"),
                evidence::suite_artifact_path(output_dir, "sqlite_parity", "ranked.csv"),
                evidence::suite_artifact_path(output_dir, "sqlite_parity", "manifest.json"),
                evidence::suite_artifact_path(output_dir, "sqlite_parity", "provenance.json"),
                &sqlite_summary,
            )
            .with_known_failures(known_failures.listed("sqlite_parity"))
            .with_completion_marker(),
            OfficialSuiteEvidence::new(
                "memory",
                memory_output,
                evidence::suite_artifact_path(output_dir, "memory", "summary.json"),
                evidence::suite_artifact_path(output_dir, "memory", "ranked.csv"),
                evidence::suite_artifact_path(output_dir, "memory", "manifest.json"),
                evidence::suite_artifact_path(output_dir, "memory", "provenance.json"),
                &memory_summary,
            )
            .with_known_failures(known_failures.listed("memory"))
            .with_completion_marker(),
            OfficialSuiteEvidence::new(
                "rql_phase1",
                rql_output,
                evidence::suite_artifact_path(output_dir, "rql_phase1", "summary.json"),
                evidence::suite_artifact_path(output_dir, "rql_phase1", "ranked.csv"),
                evidence::suite_artifact_path(output_dir, "rql_phase1", "manifest.json"),
                evidence::suite_artifact_path(output_dir, "rql_phase1", "provenance.json"),
                &rql_summary,
            )
            .with_known_failures(known_failures.listed("rql_phase1"))
            .with_completion_marker(),
            OfficialSuiteEvidence::new(
                "beyond_sqlite",
                beyond_output.clone(),
                evidence::suite_artifact_path(output_dir, "beyond_sqlite", "summary.json"),
                evidence::suite_artifact_path(output_dir, "beyond_sqlite", "ranked.csv"),
                evidence::suite_artifact_path(output_dir, "beyond_sqlite", "manifest.json"),
                evidence::suite_artifact_path(output_dir, "beyond_sqlite", "provenance.json"),
                &beyond_summary,
            )
            .without_case_ids(),
        ],
        known_failures: known_failures.source().cloned(),
        scope_policy_sha256: scope_policy.sha256().to_owned(),
        official: args.official,
        case_timeout_ms: u128::from(args.case_timeout_ms),
        max_output_bytes: args.max_output_bytes,
    })?;
    join_problems(sqlite_gates.into_iter().chain([postgres_gate]))
}

fn run_sqlite_like_suite(
    args: &RunArgs,
    suite: Suite,
    output: PathBuf,
    workers: usize,
    tmp_root: PathBuf,
    sqlite_bin: PathBuf,
) -> Result<sqlite_parity::RunSummary> {
    let memory_samples = args.memory_samples || matches!(suite, Suite::Memory);
    let started_unix_ms = evidence::now_unix_ms();
    let config = sqlite_parity::RunConfig {
        suite: suite.as_str(),
        reference_bin: sqlite_bin.clone(),
        target_bin: args.target_bin.clone(),
        output: output.clone(),
        tmp_root: tmp_root.clone(),
        workers,
        repetitions: args.repetitions,
        warmup: args.warmup,
        progress: progress_enabled(args.progress),
        memory_samples,
        case_ids: args.case_ids.clone(),
        limits: sqlite_parity::Limits::new(args.case_timeout_ms, args.max_output_bytes)?,
        official: args.official,
        order: args.order,
    };
    let summary = if matches!(suite, Suite::RqlPhase1) {
        sqlite_parity::run_rql_phase1(config)?
    } else {
        sqlite_parity::run(config)?
    };
    evidence::write_sqlite_parity_evidence(EvidenceConfig {
        suite: suite.as_str().to_owned(),
        output,
        target_bin: args.target_bin.clone(),
        sqlite_bin,
        tmp_root,
        workers: workers.to_string(),
        repetitions: args.repetitions,
        warmup: args.warmup,
        memory_samples,
        measurement_order: args.order,
        command_line: std::env::args().collect::<Vec<_>>(),
        started_unix_ms,
        ended_unix_ms: evidence::now_unix_ms(),
        summary: summary.clone(),
    })?;
    Ok(summary)
}

fn run_beyond_sqlite_suite(args: &RunArgs, output: PathBuf) -> Result<sqlite_parity::RunSummary> {
    beyond_sqlite::run(beyond_sqlite::RunConfig {
        target_bin: args.target_bin.clone(),
        output,
        command_line: std::env::args().collect::<Vec<_>>(),
        started_unix_ms: evidence::now_unix_ms(),
    })
}

fn write_all_manifest<'a>(
    output_dir: &Path,
    output: &Path,
    summaries: impl IntoIterator<Item = (&'a str, &'a Path, sqlite_parity::RunSummary)>,
) -> Result<PathBuf> {
    let suites = summaries
        .into_iter()
        .map(|(suite, raw_path, summary)| {
            serde_json::json!({
                "suite": suite,
                "raw": raw_path.display().to_string(),
                "total": summary.total,
                "passed": summary.passed,
                "failed": summary.failed,
                "skipped": summary.skipped
            })
        })
        .collect::<Vec<_>>();
    let manifest = serde_json::json!({
        "schema_version": "redline-testing-all-manifest-v1",
        "suite": "all",
        "raw": output.display().to_string(),
        "suites": suites
    });
    let manifest_path = output_dir.join("all-manifest.json");
    fs::write(
        &manifest_path,
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )
    .with_context(|| format!("write {}", manifest_path.display()))?;
    Ok(manifest_path)
}

pub(crate) fn resolve_workers(value: &str) -> Result<usize> {
    if value == "auto" {
        return Ok(std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .max(1));
    }
    let workers = value
        .parse::<usize>()
        .map_err(|_| anyhow::anyhow!("--workers must be `auto` or a positive integer"))?;
    if workers == 0 {
        bail!("--workers must be positive");
    }
    Ok(workers)
}

fn validate_samples(repetitions: usize, warmup: usize) -> Result<()> {
    if repetitions == 0 {
        bail!("--repetitions must be positive");
    }
    if warmup > 1000 {
        bail!("--warmup is unreasonably large");
    }
    Ok(())
}

pub(crate) fn prepare_output(output: &Path) -> Result<()> {
    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create output parent {}", parent.display()))?;
    }
    fs::write(output, "").with_context(|| format!("truncate output {}", output.display()))
}

fn resolve_tmp_root(raw: &str) -> Result<PathBuf> {
    if raw != "auto" {
        return Ok(PathBuf::from(raw));
    }
    if let Some(path) = std::env::var_os("REDLINE_TESTING_TMPDIR")
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path));
    }
    let shm = Path::new("/dev/shm/redline-testing");
    if is_writable_dir(shm) {
        return Ok(shm.to_path_buf());
    }
    Ok(std::env::temp_dir().join("redline-testing"))
}

fn resolve_sqlite_bin(raw: &str) -> PathBuf {
    if raw == "auto" {
        PathBuf::from(sqlite_parity::REFERENCE_CLI_BIN)
    } else {
        PathBuf::from(raw)
    }
}

pub(crate) fn progress_enabled(mode: ProgressMode) -> bool {
    match mode {
        ProgressMode::Always => true,
        ProgressMode::Never => false,
        ProgressMode::Auto => std::io::IsTerminal::is_terminal(&std::io::stderr()),
    }
}

fn is_writable_dir(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let probe = path.join(format!(".redline-testing-{}", std::process::id()));
    match fs::File::create(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}
