//! Bounded-run tests: stdin and output flow, the output cap, the deadline,
//! descendants that leave the group, and exits distinguished from signals.

use std::time::Instant;

use super::*;

fn limits(timeout_ms: u64, max_output_bytes: usize) -> Limits {
    Limits::new(timeout_ms, max_output_bytes).expect("limits")
}

fn shell(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(script);
    command
}

#[test]
fn a_child_that_never_reads_stdin_cannot_block_the_harness() {
    // Far more stdin than a pipe holds, to a child that ignores it.
    let stdin = vec![b'x'; 4 * 1024 * 1024];
    let captured =
        run_piped(&mut shell("echo done"), Some(stdin), limits(10_000, 1024)).expect("run");
    assert_eq!(captured.outcome, ExecutionOutcome::Exited);
    assert_eq!(captured.stdout, b"done\n");
}

#[test]
fn stdin_and_large_output_flow_at_once() {
    // `cat` echoes 1 MiB while it is still being fed: writing all of
    // stdin before reading would deadlock on the full pipes.
    let stdin = vec![b'y'; 1024 * 1024];
    let captured = run_piped(
        &mut shell("cat"),
        Some(stdin.clone()),
        limits(10_000, 2 << 20),
    )
    .expect("run");
    assert_eq!(captured.outcome, ExecutionOutcome::Exited);
    assert_eq!(captured.stdout, stdin);
}

#[test]
fn output_of_exactly_the_cap_is_kept_whole() {
    let captured = run_piped(&mut shell("printf 'abcd'"), None, limits(10_000, 4)).expect("run");
    assert_eq!(captured.outcome, ExecutionOutcome::Exited);
    assert_eq!(captured.stdout, b"abcd");
    let captured = run_piped(
        &mut shell("printf 'abcde'; sleep 5"),
        None,
        limits(10_000, 4),
    )
    .expect("run");
    assert_eq!(captured.outcome, ExecutionOutcome::OutputLimit);
    assert_eq!(captured.stdout, b"abcd");
}

#[test]
fn both_capture_paths_time_out_and_cap_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (out, err) = (dir.path().join("out"), dir.path().join("err"));
    let captured = run_to_files(
        &mut shell("sleep 30"),
        None,
        limits(200, 1024),
        &out,
        &err,
        true,
    )
    .expect("run");
    assert_eq!(captured.outcome, ExecutionOutcome::Timeout);
    assert!(captured.elapsed < Duration::from_secs(10), "{captured:?}");
    let captured = run_to_files(
        &mut shell("yes"),
        None,
        limits(10_000, 1024),
        &out,
        &err,
        false,
    )
    .expect("run");
    assert_eq!(captured.outcome, ExecutionOutcome::OutputLimit);
    assert_eq!(captured.stdout.len(), 1024);
    let captured = run_piped(&mut shell("sleep 30"), None, limits(200, 1024)).expect("run");
    assert_eq!(captured.outcome, ExecutionOutcome::Timeout);
    assert_eq!(captured.status.code(), None);
}

#[test]
fn file_capture_marks_output_past_the_cap_after_the_child_exits() {
    // The child writes past the cap and exits between two polls, so no
    // poll sees the oversized file while the child is still running.
    // The kept bytes stop at the cap, so the run must say it was
    // capped, as the piped path does, not pass as a whole result.
    let dir = tempfile::tempdir().expect("tempdir");
    let (out, err) = (dir.path().join("out"), dir.path().join("err"));
    for script in [
        "head -c 2048 /dev/zero",
        "head -c 2048 /dev/zero >&2",
        "sleep 0.05; head -c 2048 /dev/zero",
    ] {
        let captured = run_to_files(
            &mut shell(script),
            None,
            limits(10_000, 1024),
            &out,
            &err,
            false,
        )
        .expect("run");
        assert_eq!(
            captured.outcome,
            ExecutionOutcome::OutputLimit,
            "{script}: kept {} stdout and {} stderr bytes",
            captured.stdout.len(),
            captured.stderr.len()
        );
        assert!(!captured.outcome.is_complete());
        assert!(captured.stdout.len() <= 1024 && captured.stderr.len() <= 1024);
    }
    // Output of exactly the cap is a whole result.
    let captured = run_to_files(
        &mut shell("head -c 1024 /dev/zero"),
        None,
        limits(10_000, 1024),
        &out,
        &err,
        false,
    )
    .expect("run");
    assert_eq!(captured.outcome, ExecutionOutcome::Exited);
    assert_eq!(captured.stdout.len(), 1024);
}

/// Kills every process whose command line contains `pattern`.
fn kill_matching(pattern: &str) {
    let _ = Command::new("pkill")
        .args(["-KILL", "-f", pattern])
        .status();
}

#[test]
fn a_descendant_that_left_the_group_cannot_hold_the_harness() {
    // The leader exits at once, but a descendant in its own session
    // (outside the group the sweep kills) keeps stdout, stderr or stdin
    // open. Waiting for end of file, or for the stdin writer, would
    // block until that descendant exits; the run must instead end at
    // the case deadline as a timeout, never pass as a whole result.
    // The leader lingers so the descendant has left the group before
    // the leader exits and the sweep kills what is left of it.
    const SETTLE: &str = "sleep 0.2;";
    let mark = format!("{}{}", std::process::id(), line!());
    let pattern = format!("sleep 20.{mark}");
    for (script, stdin) in [
        (format!("setsid {pattern} & {SETTLE} echo done"), None),
        (format!("setsid {pattern} >&2 & {SETTLE} echo done"), None),
        (
            format!("exec 3<&0; setsid {pattern} <&3 >/dev/null 2>&1 & {SETTLE} echo done"),
            Some(vec![b'z'; 4 * 1024 * 1024]),
        ),
    ] {
        let started = Instant::now();
        let captured = run_piped(&mut shell(&script), stdin, limits(500, 1024)).expect("run");
        let waited = started.elapsed();
        kill_matching(&pattern);
        assert!(
            waited < Duration::from_secs(10),
            "{script}: the harness waited {waited:?} on a descendant outside the group"
        );
        assert_eq!(captured.outcome, ExecutionOutcome::Timeout, "{script}");
        assert_eq!(captured.stdout, b"done\n", "{script}");
    }
    // The file-capture path has no readers, but its stdin writer is
    // held the same way.
    let dir = tempfile::tempdir().expect("tempdir");
    let script = format!("exec 3<&0; setsid {pattern} <&3 >/dev/null 2>&1 & {SETTLE} echo done");
    let started = Instant::now();
    let captured = run_to_files(
        &mut shell(&script),
        Some(vec![b'z'; 4 * 1024 * 1024]),
        limits(500, 1024),
        &dir.path().join("out"),
        &dir.path().join("err"),
        false,
    )
    .expect("run");
    let waited = started.elapsed();
    kill_matching(&pattern);
    assert!(
        waited < Duration::from_secs(10),
        "{script}: the harness waited {waited:?} on a descendant outside the group"
    );
    assert_eq!(captured.outcome, ExecutionOutcome::Timeout, "{script}");
}

#[test]
fn exits_and_signals_are_told_apart() {
    let captured = run_piped(&mut shell("exit 3"), None, limits(10_000, 16)).expect("run");
    assert_eq!(captured.outcome, ExecutionOutcome::Exited);
    assert_eq!(captured.status.code(), Some(3));
    let captured = run_piped(&mut shell("kill -SEGV $$"), None, limits(10_000, 16)).expect("run");
    assert_eq!(captured.outcome, ExecutionOutcome::Signal);
    assert!(!ExecutionOutcome::Timeout.is_complete());
    assert!(ExecutionOutcome::Signal.is_complete());
}

#[test]
fn limits_must_be_positive() {
    assert!(Limits::new(0, 1).is_err());
    assert!(Limits::new(1, 0).is_err());
    check_kill().expect("kill works here");
}
