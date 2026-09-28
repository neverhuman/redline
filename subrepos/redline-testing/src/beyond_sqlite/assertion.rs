//! The assertion evidence a target record carries, so the gate can re-check
//! a pass rather than trust the runner's `status` (PG-06).
//!
//! The runner compares normalized stdout, exit codes, and, for a case the
//! corpus declares a rejection, the target's stderr and whether its setup
//! ran on its own. Until PG-06 the record kept only the exits, a truncated
//! stdout and the first stderr line, and the gate passed any declared case
//! whose status said "passed" -- (0,3), (3,0) or (7,7) included. The record
//! now carries what each assertion was computed from.

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::transcript::COMPARATOR_VERSION;

/// Target stderr longer than this is cut, and marked so; the gate refuses a
/// cut stderr as evidence for a declared rejection.
pub const MAX_STDERR_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct AssertionEvidence {
    /// The corpus's `expected_reference_exit` for the case.
    pub expected_reference_exit: i32,
    /// Exit code of the case's setup run alone against a fresh target, for a
    /// declared rejection that has setup; -1 if it could not be run. `None`
    /// when the case has no setup or is not a declared rejection.
    pub setup_exit_code: Option<i32>,
    /// Whether the target's stderr contains the declared error text; `None`
    /// for a case that declares none.
    pub declared_error_matched: Option<bool>,
    /// SHA-256 of each engine's complete stdout, before normalization.
    pub reference_raw_stdout_sha256: String,
    pub target_raw_stdout_sha256: String,
    /// SHA-256 of the complete normalized stdout that was compared.
    pub reference_normalized_stdout_sha256: String,
    pub target_normalized_stdout_sha256: String,
    /// The target's stderr, cut at `MAX_STDERR_BYTES`.
    pub target_stderr: String,
    pub target_stderr_truncated: bool,
    /// SHA-256 of the target's complete stderr.
    pub target_stderr_sha256: String,
    pub comparator_version: &'static str,
}

/// What the runner observed for one case, before any verdict.
pub struct Observed<'a> {
    pub expected_reference_exit: i32,
    pub reference_stdout: &'a str,
    pub target_stdout: &'a str,
    pub reference_normalized: &'a str,
    pub target_normalized: &'a str,
    pub target_stderr: &'a str,
    pub declared_error: Option<&'a str>,
    pub setup_exit_code: Option<i32>,
}

impl AssertionEvidence {
    pub fn new(observed: Observed<'_>) -> Self {
        let (target_stderr, target_stderr_truncated) =
            cut(observed.target_stderr, MAX_STDERR_BYTES);
        Self {
            expected_reference_exit: observed.expected_reference_exit,
            setup_exit_code: observed.setup_exit_code,
            declared_error_matched: observed
                .declared_error
                .map(|text| observed.target_stderr.contains(text)),
            reference_raw_stdout_sha256: sha256(observed.reference_stdout),
            target_raw_stdout_sha256: sha256(observed.target_stdout),
            reference_normalized_stdout_sha256: sha256(observed.reference_normalized),
            target_normalized_stdout_sha256: sha256(observed.target_normalized),
            target_stderr,
            target_stderr_truncated,
            target_stderr_sha256: sha256(observed.target_stderr),
            comparator_version: COMPARATOR_VERSION,
        }
    }
}

pub fn sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// `text` cut to at most `max` bytes on a character boundary, and whether it
/// was cut.
fn cut(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_owned(), false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed<'a>(stderr: &'a str, declared: Option<&'a str>) -> Observed<'a> {
        Observed {
            expected_reference_exit: 3,
            reference_stdout: "t\n",
            target_stdout: "1\n",
            reference_normalized: "1\n",
            target_normalized: "1\n",
            target_stderr: stderr,
            declared_error: declared,
            setup_exit_code: Some(0),
        }
    }

    #[test]
    fn evidence_hashes_complete_outputs_and_checks_the_declared_text() {
        let evidence = AssertionEvidence::new(observed("Error: boom\n", Some("boom")));
        assert_eq!(evidence.declared_error_matched, Some(true));
        assert_ne!(
            evidence.reference_raw_stdout_sha256,
            evidence.target_raw_stdout_sha256
        );
        assert_eq!(
            evidence.reference_normalized_stdout_sha256,
            evidence.target_normalized_stdout_sha256
        );
        assert_eq!(evidence.target_stderr_sha256, sha256("Error: boom\n"));
        assert!(!evidence.target_stderr_truncated);
        assert_eq!(evidence.comparator_version, COMPARATOR_VERSION);
        let other = AssertionEvidence::new(observed("Error: other\n", Some("boom")));
        assert_eq!(other.declared_error_matched, Some(false));
        let undeclared = AssertionEvidence::new(observed("", None));
        assert_eq!(undeclared.declared_error_matched, None);
    }

    #[test]
    fn a_long_stderr_is_cut_and_marked() {
        let long = "é".repeat(MAX_STDERR_BYTES);
        let evidence = AssertionEvidence::new(observed(&long, None));
        assert!(evidence.target_stderr_truncated);
        assert!(evidence.target_stderr.len() <= MAX_STDERR_BYTES);
        assert_eq!(evidence.target_stderr_sha256, sha256(&long));
    }
}
