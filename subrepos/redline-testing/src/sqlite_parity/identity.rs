//! Content identities for what a `sqlite_parity`-family run measured with:
//! the corpus compiled into the runner and the rules that turn two engine
//! outputs into a case verdict. Both are recorded in the run provenance and
//! official evidence (SQ-03), so a report can name them without guessing.

use sha2::{Digest, Sha256};

/// The runner sources that decide a case verdict: case fields and their
/// defaults, the execution bounds, the output comparison and normalization,
/// capability skips, the RQL phase-1 rewrite gate and the run summary. Hashing the sources means
/// any change to those rules changes the hash (unrelated edits in the same
/// files change it too; it can be too strict, never too loose).
const ASSERTION_POLICY_SOURCES: [(&str, &str); 12] = [
    ("sqlite_parity/bounded.rs", include_str!("bounded.rs")),
    (
        "sqlite_parity/bounded/files.rs",
        include_str!("bounded/files.rs"),
    ),
    (
        "sqlite_parity/bounded/group.rs",
        include_str!("bounded/group.rs"),
    ),
    (
        "sqlite_parity/bounded/piped.rs",
        include_str!("bounded/piped.rs"),
    ),
    ("sqlite_parity/case.rs", include_str!("case.rs")),
    ("sqlite_parity/compare.rs", include_str!("compare.rs")),
    ("sqlite_parity/engine.rs", include_str!("engine.rs")),
    ("sqlite_parity/normalize.rs", include_str!("normalize.rs")),
    ("sqlite_parity/rql_phase1.rs", include_str!("rql_phase1.rs")),
    ("sqlite_parity/runner.rs", include_str!("runner.rs")),
    (
        "sqlite_parity/scope_policy.rs",
        include_str!("scope_policy.rs"),
    ),
    ("sqlite_parity/text.rs", include_str!("text.rs")),
];

/// SHA-256 over every corpus file compiled into this runner (the pinned
/// manifest and every shard), each framed by its name and length.
pub fn corpus_sha256() -> String {
    framed_sha256(super::catalog::corpus_files())
}

/// SHA-256 over the verdict sources in `ASSERTION_POLICY_SOURCES`.
pub fn assertion_policy_sha256() -> String {
    framed_sha256(
        ASSERTION_POLICY_SOURCES
            .iter()
            .map(|(name, body)| ((*name).to_owned(), *body)),
    )
}

fn framed_sha256<'a>(files: impl Iterator<Item = (String, &'a str)>) -> String {
    let mut hasher = Sha256::new();
    for (name, body) in files {
        hasher.update((name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update((body.len() as u64).to_le_bytes());
        hasher.update(body.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::{assertion_policy_sha256, corpus_sha256, framed_sha256};

    #[test]
    fn corpus_and_policy_hashes_are_stable_hex_digests() {
        for hash in [corpus_sha256(), assertion_policy_sha256()] {
            assert_eq!(hash.len(), 64, "{hash}");
            assert!(hash.bytes().all(|byte| byte.is_ascii_hexdigit()), "{hash}");
        }
        assert_eq!(corpus_sha256(), corpus_sha256());
        assert_ne!(corpus_sha256(), assertion_policy_sha256());
    }

    #[test]
    fn framing_separates_names_from_contents() {
        let joined = framed_sha256([("ab".to_owned(), "c")].into_iter());
        let split = framed_sha256([("a".to_owned(), "bc")].into_iter());
        assert_ne!(joined, split);
    }
}
