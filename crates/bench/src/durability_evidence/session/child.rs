//! The child process itself: its stdout copied to a log and handed over
//! line by line, the SIGKILL and its reaping, and the busy-wait that moves
//! the kill around inside one commit.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdout, ExitStatus};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::recover::harness::describe;

use super::workload::WorkloadOutcome;

pub(super) enum Event {
    Line(String),
    Eof,
    Error(String),
}

/// SIGKILL the child, then reap it. The fault counts only when the signal
/// reached a live child and the child died of it.
pub(super) fn kill(child: &mut Child, outcome: &mut WorkloadOutcome) {
    match child.try_wait() {
        Ok(Some(status)) => {
            outcome.status = format!("exited before the kill: {}", describe(status));
            outcome.kill_result = "not sent".to_owned();
            return;
        }
        Ok(None) => {}
        Err(err) => {
            outcome.status = format!("try_wait failed: {err}");
            return;
        }
    }
    outcome.kill_result = match child.kill() {
        Ok(()) => "sent SIGKILL".to_owned(),
        Err(err) => format!("kill failed: {err}"),
    };
    match child.wait() {
        Ok(status) => {
            outcome.fault_observed =
                outcome.kill_result == "sent SIGKILL" && died_of_sigkill(status);
            outcome.status = describe(status);
        }
        Err(err) => outcome.status = format!("wait failed: {err}"),
    }
}

#[cfg(unix)]
fn died_of_sigkill(status: ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt;
    status.signal() == Some(libc::SIGKILL)
}

#[cfg(not(unix))]
fn died_of_sigkill(_status: ExitStatus) -> bool {
    false
}

/// Busy-wait: `thread::sleep` has millisecond-scale slack, and the delay
/// is what moves the kill around inside one commit.
pub(super) fn spin_for(delay: Duration) {
    let until = Instant::now() + delay;
    while Instant::now() < until {
        std::hint::spin_loop();
    }
}

/// Copy the child's stdout byte for byte to `log` and hand every line to
/// the returned receiver.
pub(super) fn read_lines(stdout: ChildStdout, mut log: File) -> (Receiver<Event>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => {
                    let _ = tx.send(Event::Eof);
                    break;
                }
                Ok(_) => {
                    if let Err(err) = log.write_all(&buf) {
                        let _ = tx.send(Event::Error(format!("write stdout log: {err}")));
                        break;
                    }
                    if buf.last() != Some(&b'\n') {
                        // A partial last line: the child died mid-write. It
                        // is logged, and it is not an ack.
                        let _ = tx.send(Event::Eof);
                        break;
                    }
                    let line = String::from_utf8_lossy(&buf[..buf.len() - 1]).into_owned();
                    if tx.send(Event::Line(line)).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    let _ = tx.send(Event::Error(err.to_string()));
                    break;
                }
            }
        }
        let _ = log.sync_all();
    });
    (rx, handle)
}
