//! A fired deadline does not prove that the leader was killed.

use std::process::ExitStatus;

use super::ExecutionOutcome;

/// Keep every exceeded deadline incomplete, while distinguishing a
/// normally exited leader from a leader terminated by a signal.
pub(super) fn expired(status: ExitStatus) -> ExecutionOutcome {
    if status.code().is_some() {
        ExecutionOutcome::DeadlineExceeded
    } else {
        ExecutionOutcome::Timeout
    }
}
