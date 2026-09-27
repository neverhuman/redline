//! Fixtures shared by the verdict tests: a UNIQUE-failure case and engine
//! outputs built from raw bytes.

use std::time::Duration;

use super::bounded::ExecutionOutcome;
use super::case::{Case, Priority, Profile};
use super::engine::EngineOutput;

/// Case 10547's shape: a duplicate insert that must fail with
/// `UNIQUE constraint failed: t.x`.
pub(super) fn unique_case() -> Case {
    Case {
        id: 10547,
        folder: "SQLITE_PARITY_10547_UNIQUE_CONSTRAINT_FAILED".to_owned(),
        name: "UNIQUE_CONSTRAINT_FAILED".to_owned(),
        category: "SQL_ERROR_MESSAGES".to_owned(),
        priority: Priority::P0,
        profile: Profile::Memory,
        kind: "sql".to_owned(),
        description: String::new(),
        status: "active".to_owned(),
        db: ":memory:".to_owned(),
        args: Vec::new(),
        stdin: "CREATE TABLE t(x UNIQUE);\nINSERT INTO t VALUES (1);\nINSERT INTO t VALUES (1);\n"
            .to_owned(),
        expected_exit: 1,
        compare_stdout: true,
        expected_stdout: None,
        expected_stdout_contains: Vec::new(),
        expected_stderr_contains: vec!["UNIQUE constraint failed: t.x".to_owned()],
        expected_combined_contains: Vec::new(),
        files: Vec::new(),
        script: None,
        notes: String::new(),
        required_capabilities: Vec::new(),
        comparison_mode: Default::default(),
        ignore_line_prefixes: Vec::new(),
        stdout_uncompared_reason: None,
    }
}

/// A case that expects exit 0 and declares nothing but its stdout
/// comparison.
pub(super) fn plain_case() -> Case {
    let mut case = unique_case();
    case.expected_exit = 0;
    case.expected_stderr_contains.clear();
    case
}

/// What one engine printed, byte for byte; a missing status code is a
/// death by signal.
pub(super) fn output(
    engine: &str,
    status_code: Option<i32>,
    stdout: impl AsRef<[u8]>,
    stderr: impl AsRef<[u8]>,
) -> EngineOutput {
    EngineOutput {
        engine: engine.to_owned(),
        executable_path: format!("/bin/{engine}"),
        executable_sha256: String::new(),
        version: String::new(),
        status_code,
        elapsed: Duration::from_millis(1),
        stdout: stdout.as_ref().to_vec(),
        stderr: stderr.as_ref().to_vec(),
        memory_status: "disabled".to_owned(),
        peak_rss_kb: None,
        rss_sampled_kb: None,
        outcome: if status_code.is_some() {
            ExecutionOutcome::Exited
        } else {
            ExecutionOutcome::Signal
        },
        failure: None,
    }
}

/// A run that left no whole result: timed out, capped or never started.
pub(super) fn incomplete(engine: &str, outcome: ExecutionOutcome, failure: &str) -> EngineOutput {
    let mut run = output(engine, None, "partial", "");
    run.outcome = outcome;
    run.failure = Some(failure.to_owned());
    run
}
