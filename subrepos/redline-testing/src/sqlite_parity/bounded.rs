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
//!   nor survive the case.
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
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::memory::ProcessMemory;

pub const DEFAULT_CASE_TIMEOUT_MS: u64 = 60_000;
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// How often the file-capture path (memory sampling) polls its child.
const POLL_INTERVAL: Duration = Duration::from_millis(2);

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
    let stdout = join_reader(stdout, "stdout")?;
    let stderr = join_reader(stderr, "stderr")?;
    if let Some(writer) = writer {
        let _ = writer.join();
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
/// files are checked on every poll.
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
    if let Some(writer) = writer {
        let _ = writer.join();
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

/// Writes all of `stdin` and closes the pipe. A child that exits or is
/// killed without reading it ends the write with a broken pipe, which is
/// not an error of the run.
fn spawn_writer(child: &mut Child, stdin: Option<Vec<u8>>) -> Option<JoinHandle<()>> {
    let bytes = stdin?;
    let mut pipe = child.stdin.take()?;
    Some(thread::spawn(move || {
        let _ = pipe.write_all(&bytes);
    }))
}

fn spawn_reader<R: Read + Send + 'static>(
    pipe: Option<R>,
    cap: usize,
    group: &Arc<Group>,
) -> JoinHandle<io::Result<Vec<u8>>> {
    let group = Arc::clone(group);
    thread::spawn(move || {
        let mut kept = Vec::new();
        let Some(mut pipe) = pipe else {
            return Ok(kept);
        };
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = match pipe.read(&mut buffer) {
                Ok(0) => return Ok(kept),
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            let room = cap.saturating_sub(kept.len());
            kept.extend_from_slice(&buffer[..read.min(room)]);
            if read > room {
                // Keep draining (and dropping) until the kill closes the
                // pipe, so the child is never left blocked on a full pipe.
                group.fire(FIRED_OUTPUT_LIMIT);
            }
        }
    })
}

fn join_reader(reader: JoinHandle<io::Result<Vec<u8>>>, stream: &str) -> Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| anyhow::anyhow!("sqlite parity {stream} reader panicked"))?
        .with_context(|| format!("read sqlite parity child {stream}"))
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
