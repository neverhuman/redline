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
//!
//! Every entry, closed ones included, must name a corpus case by its id and
//! its exact manifest name (`checked`). Until PG-04 only deferred ids were
//! checked, and with every entry closed that check covered nothing.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};

use super::case::BeyondCase;

pub const SKIP_LIST: &str = include_str!("../../metadata/beyond_sqlite/skip-list.toml");

/// One `[[skip]]` entry.
#[derive(Debug, Clone)]
pub struct Skip {
    pub case_id: String,
    /// The case's exact name in the corpus manifest.
    pub name: String,
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
    #[derive(Default)]
    struct Entry {
        case_id: Option<String>,
        name: Option<String>,
        target_release: Option<String>,
        rationale: bool,
    }
    let mut entries: Vec<Entry> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line == "[[skip]]" {
            entries.push(Entry::default());
            continue;
        }
        let Some(entry) = entries.last_mut() else {
            continue;
        };
        if line.starts_with('#') {
            continue;
        }
        if line.starts_with("case_id") {
            entry.case_id = unquote(line);
        } else if line.starts_with("name") {
            entry.name = unquote(line);
        } else if line.starts_with("target_release") {
            entry.target_release = unquote(line);
        } else if line.starts_with("rationale") {
            entry.rationale = true;
        }
    }
    let mut skips: Vec<Skip> = Vec::new();
    for entry in entries {
        let (Some(case_id), Some(name), Some(target_release)) =
            (entry.case_id, entry.name, entry.target_release)
        else {
            bail!("skip-list entry is missing case_id, name or target_release");
        };
        if !entry.rationale {
            bail!("skip-list entry {case_id} has no rationale");
        }
        if skips.iter().any(|skip| skip.case_id == case_id) {
            bail!("skip-list lists case {case_id} twice");
        }
        skips.push(Skip {
            case_id,
            name,
            target_release,
        });
    }
    if skips.is_empty() {
        bail!("skip-list has no entries; refusing to treat the whole corpus as in scope");
    }
    Ok(skips)
}

/// `parse`, then check that every entry -- closed or not -- names a corpus
/// case by its id and its exact manifest name.
pub fn checked(text: &str, cases: &[BeyondCase]) -> Result<Vec<Skip>> {
    let skips = parse(text)?;
    let names: BTreeMap<String, &str> = cases
        .iter()
        .map(|case| (case.id.to_string(), case.name.as_str()))
        .collect();
    for skip in &skips {
        match names.get(&skip.case_id) {
            None => bail!(
                "skip-list entry {} ({}) names a case the corpus lacks",
                skip.case_id,
                skip.name
            ),
            Some(name) if *name != skip.name => bail!(
                "skip-list entry {} is named {:?}, but the corpus names that case {:?}",
                skip.case_id,
                skip.name,
                name
            ),
            Some(_) => {}
        }
    }
    Ok(skips)
}

/// Corpus ids (`BEYOND-CASE-NNNNN`) that sit outside the in-scope
/// denominator, from the shipped skip-list checked against `cases`.
pub fn deferred_case_ids(cases: &[BeyondCase]) -> Result<BTreeSet<String>> {
    Ok(checked(SKIP_LIST, cases)?
        .into_iter()
        .filter(Skip::is_deferred)
        .map(|skip| format!("BEYOND-CASE-{}", skip.case_id))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Vec<BeyondCase> {
        super::super::oracle::load_cases().unwrap()
    }

    #[test]
    fn every_skip_entry_names_a_real_case_with_its_manifest_name() {
        let skips = checked(SKIP_LIST, &corpus()).unwrap();
        assert!(
            skips.len() > 100,
            "policy shrank unexpectedly: {}",
            skips.len()
        );
        // An unknown id is refused even when the entry is closed ...
        let unknown = format!(
            "{SKIP_LIST}\n[[skip]]\ncase_id = \"99999\"\nname = \"NOPE\"\nrationale = \"x\"\ntarget_release = \"closed\"\n"
        );
        let err = checked(&unknown, &corpus()).unwrap_err().to_string();
        assert!(err.contains("names a case the corpus lacks"), "{err}");
        // ... and so is a real id under a name the manifest does not use.
        let renamed = SKIP_LIST.replacen(
            "name           = \"LISTEN_BASIC\"",
            "name           = \"LISTEN_RENAMED\"",
            1,
        );
        assert_ne!(renamed, SKIP_LIST);
        let err = checked(&renamed, &corpus()).unwrap_err().to_string();
        assert!(err.contains("\"LISTEN_RENAMED\""), "{err}");
        assert!(err.contains("\"LISTEN_BASIC\""), "{err}");
    }

    #[test]
    fn a_closed_entry_leaves_the_deferred_set_but_stays_on_record() {
        let text = "\
[[skip]]
case_id        = \"20001\"
name           = \"FIRST\"
rationale      = \"no SQLite shape\"
target_release = \"deferred\"

[[skip]]
case_id        = \"20002\"
name           = \"SECOND\"
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
        assert_eq!(deferred[0].name, "FIRST");
    }

    #[test]
    fn a_malformed_policy_is_rejected_rather_than_silently_narrowed() {
        for bad in [
            // no entries at all -- would put the whole corpus in scope
            "# only comments\n",
            // missing target_release
            "[[skip]]\ncase_id = \"20001\"\nname = \"A\"\nrationale = \"x\"\n",
            // missing rationale: a deferral with no reason is not a decision
            "[[skip]]\ncase_id = \"20001\"\nname = \"A\"\ntarget_release = \"deferred\"\n",
            // missing name: nothing to check the id against
            "[[skip]]\ncase_id = \"20001\"\nrationale = \"x\"\ntarget_release = \"deferred\"\n",
            // the same case deferred twice
            "[[skip]]\ncase_id = \"20001\"\nname = \"A\"\nrationale = \"x\"\ntarget_release = \"deferred\"\n\
             [[skip]]\ncase_id = \"20001\"\nname = \"A\"\nrationale = \"y\"\ntarget_release = \"deferred\"\n",
        ] {
            assert!(parse(bad).is_err(), "should have been rejected: {bad}");
        }
    }
}
