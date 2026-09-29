//! Kernel code builds an error value only when it returns it.
//!
//! `Option::ok_or(Error::X)` constructs its argument before it knows whether
//! the option is empty, and throwing away an unused kernel `Error` is not
//! free: `Error` holds a `Box<Error>` and an `io::Error`, so dropping one is a
//! real call on the success path. On the secondary-index read path those
//! drops were about a third of the work: count queries ran 46% faster once
//! they were gone. `ok_or_else(|| Error::X)` builds the error only on
//! failure. Clippy's `or_fun_call` does not flag enum constructors, so this
//! test scans the source instead.

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
        let lines: Vec<&str> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let Some(at) = line.find(".ok_or(") else {
                continue;
            };
            let mut arg = line[at + ".ok_or(".len()..].trim_start();
            if arg.is_empty() {
                arg = lines.get(index + 1).map_or("", |next| next.trim_start());
            }
            if is_error_constructor(arg) {
                let relative = file.strip_prefix(&root).unwrap_or(file);
                eager.push(format!(
                    "{}:{}: {}",
                    relative.display(),
                    index + 1,
                    line.trim()
                ));
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
