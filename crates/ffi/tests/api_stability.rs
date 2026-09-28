//! S8-10: the stability contract in `docs/api-stability.md` matches the code.
//!
//! - The stable `rldb_*` surface is exactly what `redlinedb.h` declares: every
//!   exported `rldb_*` function is declared there and every declared one is
//!   exported.
//! - The `sqlite3_*` matrix has one row per exported `sqlite3_*` function, its
//!   Header column says whether `redlinedb.h` declares the function, and its
//!   Status is one of the documented statuses.
//!
//! Exports are read from the `extern "C" fn` definitions in `crates/ffi/src`,
//! each of which carries `#[unsafe(no_mangle)]`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const STATUSES: [&str; 6] = [
    "tested",
    "partial",
    "refuses",
    "inert",
    "untested",
    "redlinedb-only",
];

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Identifier starting at `text`, as far as it continues.
fn identifier(text: &str) -> &str {
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    &text[..end]
}

fn exported(prefix: &str) -> BTreeSet<String> {
    let mut files = Vec::new();
    rust_sources(&crate_dir().join("src"), &mut files);
    let marker = "extern \"C\" fn ";
    let mut names = BTreeSet::new();
    for file in files {
        let text = read(&file);
        for (at, _) in text.match_indices(marker) {
            let name = identifier(&text[at + marker.len()..]);
            if name.starts_with(prefix) {
                names.insert(name.to_owned());
            }
        }
    }
    assert!(!names.is_empty(), "no {prefix} exports found");
    names
}

/// Function names declared in the header: an identifier directly followed by
/// `(` that starts with `prefix`.
fn declared(header: &str, prefix: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for (at, _) in header.match_indices(prefix) {
        let before = header[..at].chars().next_back();
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let name = identifier(&header[at..]);
        if header[at + name.len()..].starts_with('(') {
            names.insert(name.to_owned());
        }
    }
    names
}

fn header() -> String {
    read(&crate_dir().join("../../contracts/c-abi/redlinedb.h"))
}

fn matrix() -> BTreeMap<String, (String, String)> {
    let doc = read(&crate_dir().join("../../docs/api-stability.md"));
    let mut rows = BTreeMap::new();
    for line in doc.lines() {
        let Some(rest) = line.strip_prefix("| `sqlite3_") else {
            continue;
        };
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        assert!(cells.len() >= 6, "matrix row has too few cells: {line}");
        let name = format!("sqlite3_{}", identifier(rest));
        let previous = rows.insert(name.clone(), (cells[2].to_owned(), cells[3].to_owned()));
        assert!(previous.is_none(), "{name} is listed twice");
    }
    rows
}

#[test]
fn rldb_exports_match_the_header() {
    let exported = exported("rldb_");
    let declared = declared(&header(), "rldb_");
    let undeclared: Vec<_> = exported.difference(&declared).collect();
    let missing: Vec<_> = declared.difference(&exported).collect();
    assert!(
        undeclared.is_empty() && missing.is_empty(),
        "exported but not in redlinedb.h: {undeclared:?}; declared but not exported: {missing:?}"
    );
}

#[test]
fn sqlite3_matrix_lists_every_export_once() {
    let exported = exported("sqlite3_");
    let rows = matrix();
    let listed: BTreeSet<String> = rows.keys().cloned().collect();
    let unlisted: Vec<_> = exported.difference(&listed).collect();
    let stale: Vec<_> = listed.difference(&exported).collect();
    assert!(
        unlisted.is_empty() && stale.is_empty(),
        "docs/api-stability.md is missing {unlisted:?} and lists non-exports {stale:?}"
    );
}

#[test]
fn sqlite3_matrix_header_and_status_columns_are_right() {
    let declared = declared(&header(), "sqlite3_");
    for (name, (in_header, status)) in matrix() {
        let expected = if declared.contains(&name) {
            "yes"
        } else {
            "no"
        };
        assert_eq!(in_header, expected, "Header column for {name}");
        assert!(
            STATUSES.contains(&status.as_str()),
            "{name} has unknown status {status:?}"
        );
    }
}
