//! The file capture path: stdout and stderr go to files while the child is
//! polled, to sample its memory and to check the deadline and the size of
//! both files.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::group::{
    FIRED_OUTPUT_LIMIT, FIRED_TIMEOUT, Group, drain_deadline, finished_by, spawn_in_group,
    spawn_writer, stdin_mode,
};
use super::{Captured, Limits, read_capped};
use crate::sqlite_parity::memory::ProcessMemory;

/// How often the file-capture path (memory sampling) polls its child.
const POLL_INTERVAL: Duration = Duration::from_millis(2);

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

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |metadata| metadata.len())
}
