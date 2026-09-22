//! The beyond-SQLite deferral policy, read from the file that documents it.
//!
//! `metadata/beyond_sqlite/skip-list.toml` records which corpus cases the
//! project has decided not to close, with a per-case rationale. Until this
//! module existed nothing read that file: "in scope" was a subtraction
//! performed by hand in each report, which is why the figure drifted between
//! documents and why 29 entries could sit marked `deferred` while passing.
//!
//! Parsed with a line scan rather than a TOML crate on purpose -- this binary
//! produces evidence, and one fewer dependency in that path is worth more than
//! the generality.

use std::collections::BTreeSet;

use anyhow::{Result, bail};

pub const SKIP_LIST: &str = include_str!("../../metadata/beyond_sqlite/skip-list.toml");

/// One `[[skip]]` entry.
#[derive(Debug, Clone)]
pub struct Skip {
    pub case_id: String,
    pub target_release: String,
}

impl Skip {
    /// `deferred` means no plan to revisit; any other value names the release
    /// that is expected to close it, or `closed` for one already closed. Only
    /// a case still awaiting work sits outside the in-scope denominator.
    pub fn is_deferred(&self) -> bool {
        self.target_release != "closed"
    }
}

fn unquote(line: &str) -> Option<String> {
    let value = line.split_once('=')?.1.trim();
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    Some(inner.to_owned())
}

/// Parse every `[[skip]]` entry, rejecting a malformed or duplicated one
/// rather than silently narrowing the policy.
pub fn parse(text: &str) -> Result<Vec<Skip>> {
    let mut skips: Vec<Skip> = Vec::new();
    let mut case_id: Option<String> = None;
    let mut target_release: Option<String> = None;
    let mut rationale = false;
    let mut in_entry = false;

    let flush = |case_id: &mut Option<String>,
                 target_release: &mut Option<String>,
                 rationale: &mut bool,
                 skips: &mut Vec<Skip>|
     -> Result<()> {
        let (Some(id), Some(release)) = (case_id.take(), target_release.take()) else {
            bail!("skip-list entry is missing case_id or target_release");
        };
        if !*rationale {
            bail!("skip-list entry {id} has no rationale");
        }
        *rationale = false;
        if skips.iter().any(|s: &Skip| s.case_id == id) {
            bail!("skip-list lists case {id} twice");
        }
        skips.push(Skip {
            case_id: id,
            target_release: release,
        });
        Ok(())
    };

    for line in text.lines() {
        let line = line.trim();
        if line == "[[skip]]" {
            if in_entry {
                flush(
                    &mut case_id,
                    &mut target_release,
                    &mut rationale,
                    &mut skips,
                )?;
            }
            in_entry = true;
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        if line.starts_with("case_id") {
            case_id = unquote(line);
        } else if line.starts_with("target_release") {
            target_release = unquote(line);
        } else if line.starts_with("rationale") {
            rationale = true;
        }
    }
    if in_entry {
        flush(
            &mut case_id,
            &mut target_release,
            &mut rationale,
            &mut skips,
        )?;
    }
    if skips.is_empty() {
        bail!("skip-list has no entries; refusing to treat the whole corpus as in scope");
    }
    Ok(skips)
}

/// Corpus ids (`BEYOND-CASE-NNNNN`) that sit outside the in-scope denominator.
pub fn deferred_case_ids() -> Result<BTreeSet<String>> {
    Ok(parse(SKIP_LIST)?
        .into_iter()
        .filter(Skip::is_deferred)
        .map(|skip| format!("BEYOND-CASE-{}", skip.case_id))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_policy_parses_and_covers_only_real_cases() {
        let skips = parse(SKIP_LIST).unwrap();
        assert!(
            skips.len() > 100,
            "policy shrank unexpectedly: {}",
            skips.len()
        );
        let corpus: BTreeSet<String> = super::super::oracle::load_cases()
            .unwrap()
            .into_iter()
            .map(|case| format!("BEYOND-CASE-{:05}", case.id))
            .collect();
        for id in deferred_case_ids().unwrap() {
            assert!(
                corpus.contains(&id),
                "skip-list names a case the corpus lacks: {id}"
            );
        }
    }

    #[test]
    fn a_closed_entry_leaves_the_deferred_set_but_stays_on_record() {
        let text = "\
[[skip]]
case_id        = \"20001\"
rationale      = \"no SQLite shape\"
target_release = \"deferred\"

[[skip]]
case_id        = \"20002\"
rationale      = \"closed by the identity work\"
target_release = \"closed\"
";
        let skips = parse(text).unwrap();
        assert_eq!(
            skips.len(),
            2,
            "a closed entry keeps its rationale on record"
        );
        let deferred: Vec<_> = skips.iter().filter(|s| s.is_deferred()).collect();
        assert_eq!(deferred.len(), 1);
        assert_eq!(deferred[0].case_id, "20001");
    }

    #[test]
    fn a_malformed_policy_is_rejected_rather_than_silently_narrowed() {
        for bad in [
            // no entries at all -- would put the whole corpus in scope
            "# only comments\n",
            // missing target_release
            "[[skip]]\ncase_id = \"20001\"\nrationale = \"x\"\n",
            // missing rationale: a deferral with no reason is not a decision
            "[[skip]]\ncase_id = \"20001\"\ntarget_release = \"deferred\"\n",
            // the same case deferred twice
            "[[skip]]\ncase_id = \"20001\"\nrationale = \"x\"\ntarget_release = \"deferred\"\n\
             [[skip]]\ncase_id = \"20001\"\nrationale = \"y\"\ntarget_release = \"deferred\"\n",
        ] {
            assert!(parse(bad).is_err(), "should have been rejected: {bad}");
        }
    }
}
