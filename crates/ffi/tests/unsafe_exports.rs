//! S8-03/S9-02: every C export that takes a raw pointer is `unsafe`.
//!
//! A safe `pub extern "C" fn` that dereferences a pointer argument lets safe
//! Rust (this crate is also built as an rlib) cause undefined behaviour with
//! no `unsafe` block at the call site. Clippy's `not_unsafe_ptr_arg_deref`
//! catches only direct dereferences in the body, not the ones routed through
//! helpers such as `with_db`, and CI does not run clippy, so this test reads
//! the crate source and enforces the rule itself:
//!
//! - an exported function (`#[unsafe(no_mangle)]`) whose top-level parameter
//!   list contains a `*mut` or `*const` type must be `unsafe extern "C"`;
//! - every exported `unsafe extern "C" fn` carries a `# Safety` doc section;
//! - the crate-wide `allow(clippy::not_unsafe_ptr_arg_deref)` stays gone.

use std::fs;
use std::path::{Path, PathBuf};

struct Export {
    location: String,
    name: String,
    is_unsafe: bool,
    pointer_params: Vec<String>,
    has_safety_doc: bool,
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .expect("read src dir")
        .map(|entry| entry.expect("dir entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Split a parameter list at top-level commas, ignoring commas nested in
/// `()`, `<>` or `[]` (callback types carry their own parameter lists).
fn top_level_params(params: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    let mut previous = '\0';
    for ch in params.chars() {
        match ch {
            '(' | '<' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            // `->` inside a callback type is not a closing angle bracket.
            '>' if previous != '-' => depth -= 1,
            ',' if depth == 0 => {
                out.push(current.trim().to_string());
                current.clear();
                previous = ch;
                continue;
            }
            _ => {}
        }
        current.push(ch);
        previous = ch;
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

fn exports_in(path: &Path) -> Vec<Export> {
    let src = fs::read_to_string(path).expect("read source");
    let lines: Vec<&str> = src.lines().collect();
    let mut exports = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let (is_unsafe, rest) =
            if let Some(rest) = trimmed.strip_prefix("pub unsafe extern \"C\" fn ") {
                (true, rest)
            } else if let Some(rest) = trimmed.strip_prefix("pub extern \"C\" fn ") {
                (false, rest)
            } else {
                continue;
            };
        // Walk up over attributes and doc comments.
        let mut exported = false;
        let mut docs = Vec::new();
        let mut above = index;
        while above > 0 {
            let prev = lines[above - 1].trim_start();
            if prev.starts_with("#[") {
                exported |= prev.starts_with("#[unsafe(no_mangle)]");
            } else if let Some(doc) = prev.strip_prefix("///") {
                docs.push(doc.trim().to_string());
            } else {
                break;
            }
            above -= 1;
        }
        if !exported {
            continue;
        }
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        // Collect the balanced parameter list, which may span lines.
        let from = lines[index..].join("\n");
        let open = from.find('(').expect("parameter list");
        let mut depth = 0;
        let mut close = open;
        for (offset, ch) in from[open..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = open + offset;
                        break;
                    }
                }
                _ => {}
            }
        }
        let pointer_params = top_level_params(&from[open + 1..close])
            .into_iter()
            .filter(|param| {
                let ty = param.split_once(':').map_or("", |(_, ty)| ty.trim());
                ty.starts_with("*mut ") || ty.starts_with("*const ")
            })
            .collect();
        exports.push(Export {
            location: format!("{}:{}", path.display(), index + 1),
            name,
            is_unsafe,
            pointer_params,
            has_safety_doc: docs.iter().any(|doc| doc == "# Safety"),
        });
    }
    exports
}

fn all_exports() -> Vec<Export> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_sources(&src, &mut files);
    files.iter().flat_map(|path| exports_in(path)).collect()
}

#[test]
fn pointer_taking_exports_are_unsafe() {
    let exports = all_exports();
    assert!(
        exports.len() > 150,
        "scanner found only {} exports; the source layout changed",
        exports.len()
    );
    let offenders: Vec<String> = exports
        .iter()
        .filter(|export| !export.is_unsafe && !export.pointer_params.is_empty())
        .map(|export| {
            format!(
                "{} {}({})",
                export.location,
                export.name,
                export.pointer_params.join(", ")
            )
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "{} safe `pub extern \"C\" fn` exports take raw pointers; make them \
         `pub unsafe extern \"C\" fn` with a `# Safety` section:\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}

#[test]
fn unsafe_exports_document_safety() {
    let missing: Vec<String> = all_exports()
        .iter()
        .filter(|export| export.is_unsafe && !export.has_safety_doc)
        .map(|export| format!("{} {}", export.location, export.name))
        .collect();
    assert!(
        missing.is_empty(),
        "unsafe exports without a `# Safety` doc section:\n{}",
        missing.join("\n")
    );
}

#[test]
fn crate_does_not_allow_unsafe_pointer_derefs_in_safe_fns() {
    let lib = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
        .expect("read lib.rs");
    assert!(
        !lib.contains("not_unsafe_ptr_arg_deref"),
        "src/lib.rs must not allow clippy::not_unsafe_ptr_arg_deref crate-wide"
    );
}
