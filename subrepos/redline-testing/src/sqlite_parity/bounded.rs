//! Bounded engine runs (SQ-09). Every child a case starts runs in its own
//! process group, under a deadline and an output cap, and nothing it leaves
//! behind outlives its run:
//!
//! * stdin is written by its own thread, and stdout and stderr are drained
//!   by reader threads while the child runs, so neither a child that never
//!   reads its input nor one that writes more than a pipe holds can
//!   deadlock the harness;
//! * a reader keeps at most `max_output_bytes` of a stream; the byte after
//!   that kills the group (`output_limit`). A watchdog kills the group at
//!   the deadline (`timeout`);
//! * once the child has exited, whatever is left of its group is killed
//!   too, so a background process it started can neither hold a pipe open
//!   nor survive the case;
//! * a descendant that left the group (a new session) escapes that kill,
//!   so the streams and the stdin writer get only until the case deadline
//!   to finish; a pipe still held open then makes the run a `timeout`.
//!
//! The elapsed time is taken when the child exits, before any of that
//! cleanup, so bounding a run does not change what it measures.
//!
//! The crate takes no libc dependency, so a group is signalled with the
//! `kill` utility (`kill -s KILL -- -<pgid>`); `check_kill` proves it works
//! before a run starts.

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::memory::ProcessMemory;

pub const DEFAULT_CASE_TIMEOUT_MS: u64 = 60_000;
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// How often the file-capture path (memory sampling) polls its child.
const POLL_INTERVAL: Duration = Duration::from_millis(2);

/// The least time the streams and the stdin writer get to finish once the
/// child has exited, even when it exited just before the case deadline.
const DRAIN_GRACE: Duration = Duration::from_millis(200);

/// The bounds every engine run of a case is held to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_millis(DEFAULT_CASE_TIMEOUT_MS),
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

impl Limits {
    pub fn new(timeout_ms: u64, max_output_bytes: usize) -> Result<Self> {
        if timeout_ms == 0 {
            bail!("--case-timeout-ms must be positive");
        }
        if max_output_bytes == 0 {
            bail!("--max-output-bytes must be positive");
        }
        Ok(Self {
            timeout: Duration::from_millis(timeout_ms),
            max_output_bytes,
        })
    }

    pub fn timeout_ms(&self) -> u128 {
        self.timeout.as_millis()
    }
}

/// How one engine run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionOutcome {
    /// The child exited on its own with a status code.
    Exited,
    /// The child died of a signal the harness did not send.
    Signal,
    /// The harness killed the child's process group at the deadline.
    Timeout,
    /// The harness killed the child's process group when stdout or stderr
    /// passed the output cap; the kept bytes stop at the cap.
    OutputLimit,
    /// The run could not be prepared, spawned or waited for.
    SpawnError,
    /// Nothing ran: the case was skipped or rejected at selection.
    NotRun,
}

impl ExecutionOutcome {
    /// Whether the run left a whole result the case's contract can judge.
    /// A timed-out, capped or unstarted run did not.
    pub fn is_complete(self) -> bool {
        matches!(self, Self::Exited | Self::Signal)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exited => "exited",
            Self::Signal => "signal",
            Self::Timeout => "timeout",
            Self::OutputLimit => "output_limit",
            Self::SpawnError => "spawn_error",
            Self::NotRun => "not_run",
        }
    }
}

/// What one bounded run left.
#[derive(Debug)]
pub struct Captured {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub outcome: ExecutionOutcome,
    /// From just before the spawn to the child's exit.
    pub elapsed: Duration,
    pub memory: ProcessMemory,
}

/// Fails unless `kill -s 0 -- <this process>` works: without the `kill`
/// utility no case could be bounded, and a run must not start unbounded.
pub fn check_kill() -> Result<()> {
    let status = Command::new("kill")
        .args(["-s", "0", "--", &std::process::id().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("run `kill`, which bounds every case's process group")?;
    if !status.success() {
        bail!("`kill -s 0 -- <own pid>` exited {status}; cases cannot be bounded");
    }
    Ok(())
}

/// Runs `command` with piped stdin, stdout and stderr under `limits`.
pub fn run_piped(
    command: &mut Command,
    stdin: Option<Vec<u8>>,
    limits: Limits,
) -> Result<Captured> {
    command
        .stdin(stdin_mode(&stdin))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let started = Instant::now();
    let mut child = spawn_in_group(command)?;
    let group = Arc::new(Group::new(child.id()));
    let writer = spawn_writer(&mut child, stdin);
    let stdout = spawn_reader(child.stdout.take(), limits.max_output_bytes, &group);
    let stderr = spawn_reader(child.stderr.take(), limits.max_output_bytes, &group);
    let (exited, exited_rx) = mpsc::channel::<()>();
    let watchdog = {
        let group = Arc::clone(&group);
        thread::spawn(move || {
            if exited_rx.recv_timeout(limits.timeout) == Err(mpsc::RecvTimeoutError::Timeout) {
                group.fire(FIRED_TIMEOUT);
            }
        })
    };
    let waited = child.wait();
    let elapsed = started.elapsed();
    drop(exited);
    let _ = watchdog.join();
    group.sweep();
    // A descendant that left the group escapes the sweep and may still hold
    // a pipe. The streams and the stdin writer get until the case deadline
    // to finish; past it, the run is a timeout and they are left behind.
    let deadline = drain_deadline(started, limits);
    let (stdout, stdout_held) = join_reader(stdout, deadline, "stdout")?;
    let (stderr, stderr_held) = join_reader(stderr, deadline, "stderr")?;
    let writer_held = writer.is_some_and(|writer| !finished_by(&writer, deadline));
    if stdout_held || stderr_held || writer_held {
        group.mark(FIRED_TIMEOUT);
    }
    let status = waited.context("wait for sqlite parity child")?;
    Ok(Captured {
        outcome: group.outcome(status),
        status,
        stdout,
        stderr,
        elapsed,
        memory: ProcessMemory::default(),
    })
}

/// Runs `command` with stdout and stderr sent to files, polling it to
/// sample its memory, under `limits`: the deadline and the size of both
/// files are checked on every poll, and the size once more after the child
/// exits.
pub fn run_to_files(
    command: &mut Command,
    stdin: Option<Vec<u8>>,
    limits: Limits,
    stdout_path: &Path,
    stderr_path: &Path,
    memory_samples: bool,
) -> Result<Captured> {
    command.stdin(stdin_mode(&stdin));
    command.stdout(Stdio::from(
        fs::File::create(stdout_path)
            .with_context(|| format!("create {}", stdout_path.display()))?,
    ));
    command.stderr(Stdio::from(
        fs::File::create(stderr_path)
            .with_context(|| format!("create {}", stderr_path.display()))?,
    ));
    let started = Instant::now();
    let mut child = spawn_in_group(command)?;
    let group = Group::new(child.id());
    let writer = spawn_writer(&mut child, stdin);
    let mut memory = ProcessMemory::default();
    let waited = loop {
        if memory_samples {
            memory.observe_pid(child.id());
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(error) => break Err(error),
        }
        if started.elapsed() >= limits.timeout {
            group.fire(FIRED_TIMEOUT);
        } else if [stdout_path, stderr_path]
            .iter()
            .any(|path| file_len(path) > limits.max_output_bytes as u64)
        {
            group.fire(FIRED_OUTPUT_LIMIT);
        }
        thread::sleep(POLL_INTERVAL);
    };
    let elapsed = started.elapsed();
    if waited.is_err() {
        // The child may still be running; never leave it behind.
        let _ = child.kill();
        let _ = child.wait();
    }
    group.sweep();
    if let Some(writer) = writer
        && !finished_by(&writer, drain_deadline(started, limits))
    {
        // A descendant outside the group holds stdin without reading it.
        group.mark(FIRED_TIMEOUT);
    }
    // A child that writes past the cap and exits between two polls was
    // never seen over it: the files, not the polls, say whether the kept
    // bytes stop short of what it wrote.
    if [stdout_path, stderr_path]
        .iter()
        .any(|path| file_len(path) > limits.max_output_bytes as u64)
    {
        group.mark(FIRED_OUTPUT_LIMIT);
    }
    let status = waited.context("poll sqlite parity child")?;
    Ok(Captured {
        outcome: group.outcome(status),
        status,
        stdout: read_capped(stdout_path, limits.max_output_bytes)?,
        stderr: read_capped(stderr_path, limits.max_output_bytes)?,
        elapsed,
        memory,
    })
}

const NOT_FIRED: u8 = 0;
const FIRED_TIMEOUT: u8 = 1;
const FIRED_OUTPUT_LIMIT: u8 = 2;

/// A child's process group (its pgid is the child's pid) and the one kill
/// the harness may send it before the child exits.
struct Group {
    pgid: u32,
    fired: AtomicU8,
}

impl Group {
    fn new(pgid: u32) -> Self {
        Self {
            pgid,
            fired: AtomicU8::new(NOT_FIRED),
        }
    }

    /// Kills the group, and its leader in case it left the group, for
    /// `reason`. Only the first reason counts.
    fn fire(&self, reason: u8) {
        if self
            .fired
            .compare_exchange(NOT_FIRED, reason, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            signal_kill(&[format!("-{}", self.pgid), self.pgid.to_string()]);
        }
    }

    /// Records `reason` without signalling: the leader was already reaped,
    /// so its pid may name another process by now. Only the first reason
    /// counts.
    fn mark(&self, reason: u8) {
        let _ = self
            .fired
            .compare_exchange(NOT_FIRED, reason, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// After the leader exited: kills whatever is left of its group. The
    /// leader was reaped, so a group that is already empty makes `kill`
    /// report "no such process", which is the expected case.
    fn sweep(&self) {
        signal_kill(&[format!("-{}", self.pgid)]);
    }

    fn outcome(&self, status: ExitStatus) -> ExecutionOutcome {
        match self.fired.load(Ordering::SeqCst) {
            FIRED_TIMEOUT => ExecutionOutcome::Timeout,
            FIRED_OUTPUT_LIMIT => ExecutionOutcome::OutputLimit,
            _ if status.code().is_some() => ExecutionOutcome::Exited,
            _ => ExecutionOutcome::Signal,
        }
    }
}

fn signal_kill(targets: &[String]) {
    let _ = Command::new("kill")
        .args(["-s", "KILL", "--"])
        .args(targets)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn stdin_mode(stdin: &Option<Vec<u8>>) -> Stdio {
    if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    }
}

fn spawn_in_group(command: &mut Command) -> Result<Child> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn().context("spawn sqlite parity child")
}

/// When the streams and the stdin writer must have finished: the case
/// deadline, or `DRAIN_GRACE` from now if that is later.
fn drain_deadline(started: Instant, limits: Limits) -> Instant {
    (started + limits.timeout).max(Instant::now() + DRAIN_GRACE)
}

/// Writes all of `stdin` and closes the pipe. A child that exits or is
/// killed without reading it ends the write with a broken pipe, which is
/// not an error of the run. The receiver hears once the write is over.
fn spawn_writer(child: &mut Child, stdin: Option<Vec<u8>>) -> Option<mpsc::Receiver<()>> {
    let bytes = stdin?;
    let mut pipe = child.stdin.take()?;
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let _ = pipe.write_all(&bytes);
        drop(pipe);
        let _ = done.send(());
    });
    Some(finished)
}

/// Whether the stdin writer finished by `deadline`. A writer still blocked
/// then is left behind: nothing in the group reads its pipe any more.
fn finished_by(writer: &mpsc::Receiver<()>, deadline: Instant) -> bool {
    writer
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .is_ok()
}

/// A stream being drained: the bytes kept so far, and the reader's end.
struct Reader {
    kept: Arc<Mutex<Vec<u8>>>,
    finished: mpsc::Receiver<io::Result<()>>,
}

fn spawn_reader<R: Read + Send + 'static>(
    pipe: Option<R>,
    cap: usize,
    group: &Arc<Group>,
) -> Reader {
    let group = Arc::clone(group);
    let kept = Arc::new(Mutex::new(Vec::new()));
    let (done, finished) = mpsc::channel();
    let shared = Arc::clone(&kept);
    thread::spawn(move || {
        let _ = done.send(drain(pipe, cap, &group, &shared));
    });
    Reader { kept, finished }
}

fn drain<R: Read>(
    pipe: Option<R>,
    cap: usize,
    group: &Group,
    kept: &Mutex<Vec<u8>>,
) -> io::Result<()> {
    let Some(mut pipe) = pipe else {
        return Ok(());
    };
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = match pipe.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        let overflow = {
            let mut kept = kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let room = cap.saturating_sub(kept.len());
            kept.extend_from_slice(&buffer[..read.min(room)]);
            read > room
        };
        if overflow {
            // Keep draining (and dropping) until the kill closes the
            // pipe, so the child is never left blocked on a full pipe.
            group.fire(FIRED_OUTPUT_LIMIT);
        }
    }
}

/// The bytes a stream kept, and whether its pipe was still held open at
/// `deadline` (its reader is then left behind with what it kept so far).
fn join_reader(reader: Reader, deadline: Instant, stream: &str) -> Result<(Vec<u8>, bool)> {
    let held = match reader
        .finished
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
    {
        Ok(result) => {
            result.with_context(|| format!("read sqlite parity child {stream}"))?;
            false
        }
        Err(mpsc::RecvTimeoutError::Timeout) => true,
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            bail!("sqlite parity {stream} reader panicked")
        }
    };
    let kept = reader
        .kept
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    Ok((kept, held))
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |metadata| metadata.len())
}

/// The first `cap` bytes of a captured stream.
pub(super) fn read_capped(path: &Path, cap: usize) -> Result<Vec<u8>> {
    let file = fs::File::open(path).with_context(|| format!("read {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(cap as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {}", path.display()))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
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
        let captured =
            run_piped(&mut shell("printf 'abcd'"), None, limits(10_000, 4)).expect("run");
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
        let script =
            format!("exec 3<&0; setsid {pattern} <&3 >/dev/null 2>&1 & {SETTLE} echo done");
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
        let captured =
            run_piped(&mut shell("kill -SEGV $$"), None, limits(10_000, 16)).expect("run");
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
}
