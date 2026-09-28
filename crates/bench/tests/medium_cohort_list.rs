//! The restored 294-case medium cohort (L-05) is the exact list the
//! historical README rows were measured on, and it stays byte-identical.
//!
//! `bench/perf/cases/medium-set.txt` is restored verbatim from
//! `git show 0f831a2bb^:bench/perf/cases/medium-set.txt` (written by the
//! retired scripts/perf/build_case_lists.py in 27132c4a3). A release bench
//! bundle slices its medium-cohort statistics through this file and records
//! its SHA-256, so an edited list would silently describe another cohort.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

/// The SHA-256 of the list as it stood in 0f831a2bb^.
const HISTORICAL_SHA256: &str = "df81a4b58a1a7ef4d9beafd620288f14399590cbd6fc8250e64fc2d8280b7bd7";

fn cases_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bench/perf/cases")
}

fn read(name: &str) -> Vec<u8> {
    let path = cases_dir().join(name);
    fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

#[test]
fn medium_set_is_the_historical_list_and_its_digest_file_names_it() {
    let bytes = read("medium-set.txt");
    let digest = format!("{:x}", Sha256::digest(&bytes));
    assert_eq!(
        digest, HISTORICAL_SHA256,
        "medium-set.txt differs from 0f831a2bb^:bench/perf/cases/medium-set.txt"
    );
    let recorded = String::from_utf8(read("medium-set.txt.sha256")).expect("utf-8 digest file");
    assert_eq!(
        recorded,
        format!("{HISTORICAL_SHA256}  medium-set.txt\n"),
        "medium-set.txt.sha256 must be `sha256sum medium-set.txt` output"
    );
}

#[test]
fn medium_set_lists_294_distinct_cases_in_its_documented_strata() {
    let text = String::from_utf8(read("medium-set.txt")).expect("utf-8 case list");
    assert!(
        text.contains("# corpus_size=1127 cases_selected=294"),
        "the header records the corpus it was drawn from"
    );
    let mut ids = BTreeSet::new();
    let mut strata = BTreeMap::<String, usize>::new();
    for line in text.lines() {
        let (id, comment) = line.split_once('#').unwrap_or((line, ""));
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
        assert!(
            id.len() == 5 && id.bytes().all(|byte| byte.is_ascii_digit()),
            "{line:?} is not a five-digit case id"
        );
        assert!(ids.insert(id.to_owned()), "case {id} is listed twice");
        let stratum = comment.split_whitespace().next().unwrap_or("").to_owned();
        *strata.entry(stratum).or_default() += 1;
    }
    assert_eq!(ids.len(), 294);
    assert_eq!(
        strata,
        BTreeMap::from([
            ("P0".to_owned(), 130),
            ("P1-worst".to_owned(), 100),
            ("cat-spread".to_owned(), 45),
            ("abs-outlier".to_owned(), 19),
        ])
    );
    assert!(
        ids.iter().all(|id| id.as_str() <= "01127"),
        "every case comes from the 1,127-case pinned corpus"
    );
}
