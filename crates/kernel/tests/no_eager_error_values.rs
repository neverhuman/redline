//! Kernel code does not build an error value for `Option::ok_or` before it
//! knows the value is needed.
//!
//! `ok_or(Error::X)` constructs its argument even when the option holds a
//! value, and throwing away an unused kernel `Error` is not free: `Error`
//! holds a `Box<Error>` and an `io::Error`, so dropping one is a real call
//! on the success path. The performance audit's profile of reads had
//! `drop_in_place<Error>` at 8-11% of self time; with these calls gone, the
//! engine scoreboard measured secondary-index count queries 1.4x as fast.
//! `ok_or_else(|| Error::X)` builds the error only on failure. Clippy's
//! `or_fun_call` does not flag enum constructors, so this test scans the
//! source instead.

use std::fs;
use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read source directory") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// `text` with leading whitespace and `//` comment lines removed.
fn skip_space_and_comments(mut text: &str) -> &str {
    loop {
        text = text.trim_start();
        match text.strip_prefix("//") {
            Some(comment) => text = comment.split_once('\n').map_or("", |(_, rest)| rest),
            None => return text,
        }
    }
}

/// Whether `arg` starts with a path to an error value: `Error::X`,
/// `crate::Error::X`, `RecordError::X` and the like.
fn is_error_constructor(arg: &str) -> bool {
    let path: String = arg
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
        .collect();
    path.contains("::") && path.split("::").any(|segment| segment.ends_with("Error"))
}

#[test]
fn no_kernel_code_builds_an_error_it_may_not_return() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    files.sort();
    let mut eager = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file).expect("read source file");
        for (at, _) in text.match_indices(".ok_or(") {
            let arg = skip_space_and_comments(&text[at + ".ok_or(".len()..]);
            if is_error_constructor(arg) {
                let line = text[..at].matches('\n').count() + 1;
                let relative = file.strip_prefix(&root).unwrap_or(file);
                eager.push(format!("{}:{line}", relative.display()));
            }
        }
    }
    assert!(
        eager.is_empty(),
        "{} kernel error value(s) built before they are needed; write ok_or_else(|| ...):\n{}",
        eager.len(),
        eager.join("\n")
    );
}

#[test]
fn the_scan_sees_every_occurrence_and_steps_over_comments() {
    let arg = skip_space_and_comments("\n    // why\n    Error::CorruptPage(\"x\"))");
    assert!(is_error_constructor(arg));
    let text = "a.ok_or(value).b.ok_or(crate::Error::X)";
    let hits = text
        .match_indices(".ok_or(")
        .filter(|(at, _)| is_error_constructor(skip_space_and_comments(&text[at + 7..])))
        .count();
    assert_eq!(hits, 1);
}
