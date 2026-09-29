//! The piped capture path: stdin is written by its own thread while
//! stdout and stderr are drained by reader threads, each keeping at most
//! the output cap.

use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result, bail};

use super::group::{
    FIRED_OUTPUT_LIMIT, FIRED_TIMEOUT, Group, drain_deadline, finished_by, spawn_in_group,
    spawn_writer, stdin_mode,
};
use super::{Captured, Limits};
use crate::sqlite_parity::memory::ProcessMemory;

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
