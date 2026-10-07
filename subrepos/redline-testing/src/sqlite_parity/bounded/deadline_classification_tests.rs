//! Regression fixtures also run against the parent Group implementation.

use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

use super::{FIRED_TIMEOUT, Group};

#[test]
fn successful_leader_after_deadline_still_fails_without_claiming_a_kill() {
    let group = Group::new(std::process::id());
    // mark records the already-exceeded deadline without signalling any PID.
    group.mark(FIRED_TIMEOUT);
    let outcome = group.outcome(ExitStatus::from_raw(0));
    assert_eq!(outcome.as_str(), "deadline_exceeded");
    assert!(!outcome.is_complete());
}

#[test]
fn nonzero_leader_after_deadline_is_also_incomplete() {
    let group = Group::new(std::process::id());
    group.mark(FIRED_TIMEOUT);
    let outcome = group.outcome(ExitStatus::from_raw(3 << 8));
    assert_eq!(outcome.as_str(), "deadline_exceeded");
    assert!(!outcome.is_complete());
}

#[test]
fn signal_terminated_leader_keeps_the_timeout_classification() {
    let group = Group::new(std::process::id());
    group.mark(FIRED_TIMEOUT);
    let outcome = group.outcome(ExitStatus::from_raw(9));
    assert_eq!(outcome.as_str(), "timeout");
    assert!(!outcome.is_complete());
}
