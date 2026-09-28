//! Certification refuses an unoptimized benchmark (BM3-05).
//!
//! certify's measured children exec the parent binary (the scheduler
//! dispatches `current_exe`), so a parent built without optimization
//! times unoptimized children, and its numbers say nothing about the
//! engine users run. A debug build therefore refuses to certify unless
//! `--allow-debug-build` marks the run as a diagnostic; its manifest then
//! records `build_profile: debug` and `publishable: false`.

use anyhow::{Result, bail};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildProfile {
    Release,
    Debug,
}

impl BuildProfile {
    /// Only an optimized build's numbers may be published.
    pub fn publishable(self) -> bool {
        self == Self::Release
    }
}

/// The profile this certify runs under, or the refusal of a debug build
/// that was not marked as a diagnostic.
pub fn check_build_profile(
    debug_assertions: bool,
    allow_debug_build: bool,
) -> Result<BuildProfile> {
    match (debug_assertions, allow_debug_build) {
        (false, _) => Ok(BuildProfile::Release),
        (true, true) => Ok(BuildProfile::Debug),
        (true, false) => bail!(
            "certify refuses to run from a debug build: its measured children exec this same unoptimized binary. \
             Run `cargo run -p redlinedb-bench --release -- certify ...`, or pass --allow-debug-build for a \
             diagnostic run whose manifest records build_profile debug and publishable false"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{BuildProfile, check_build_profile};

    #[test]
    fn debug_build_is_refused_unless_marked_diagnostic() {
        assert_eq!(
            check_build_profile(false, false).unwrap(),
            BuildProfile::Release
        );
        assert_eq!(
            check_build_profile(false, true).unwrap(),
            BuildProfile::Release
        );
        assert_eq!(
            check_build_profile(true, true).unwrap(),
            BuildProfile::Debug
        );
        let error = check_build_profile(true, false).unwrap_err();
        assert!(error.to_string().contains("debug build"), "{error}");
        assert!(BuildProfile::Release.publishable());
        assert!(!BuildProfile::Debug.publishable());
    }
}
