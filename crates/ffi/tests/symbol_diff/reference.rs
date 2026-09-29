//! The reference symbol set: the `sqlite3.h` bundled with `libsqlite3-sys`,
//! found through `cargo metadata`, and the `sqlite3_*` names its `SQLITE_API`
//! declarations carry.

use super::*;

pub(super) fn load_reference_from_bundled_header(workspace: &Path) -> (PathBuf, BTreeSet<String>) {
    let manifest_path = libsqlite3_sys_manifest_path(workspace);
    let header_path = manifest_path
        .parent()
        .expect("libsqlite3-sys manifest directory")
        .join("sqlite3")
        .join("sqlite3.h");
    let text = std::fs::read_to_string(&header_path).unwrap_or_else(|err| {
        panic!(
            "missing bundled sqlite3.h at {}: {err}",
            header_path.display()
        )
    });
    (header_path, parse_sqlite_api_symbols(&text))
}

fn libsqlite3_sys_manifest_path(workspace: &Path) -> PathBuf {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--locked"])
        .env("RUSTC_WRAPPER", "")
        .current_dir(workspace)
        .output()
        .expect("cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("cargo metadata stdout is UTF-8");
    let marker = "\"name\":\"libsqlite3-sys\"";
    let package_start = stdout
        .find(marker)
        .unwrap_or_else(|| panic!("libsqlite3-sys package missing from cargo metadata"));
    let manifest_marker = "\"manifest_path\":\"";
    let manifest_start = stdout[package_start..]
        .find(manifest_marker)
        .map(|offset| package_start + offset + manifest_marker.len())
        .unwrap_or_else(|| panic!("libsqlite3-sys manifest_path missing from cargo metadata"));
    let manifest_end = stdout[manifest_start..]
        .find('"')
        .map(|offset| manifest_start + offset)
        .expect("libsqlite3-sys manifest_path closing quote");
    PathBuf::from(&stdout[manifest_start..manifest_end])
}

fn parse_sqlite_api_symbols(header: &str) -> BTreeSet<String> {
    let stripped = strip_c_comments(header);
    stripped
        .split(';')
        .filter_map(extract_sqlite_api_symbol)
        .collect()
}

fn strip_c_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '/' {
            match chars.peek().copied() {
                Some('*') => {
                    chars.next();
                    while let Some(block_ch) = chars.next() {
                        if block_ch == '*' && chars.peek().copied() == Some('/') {
                            chars.next();
                            break;
                        }
                    }
                    out.push(' ');
                    continue;
                }
                Some('/') => {
                    chars.next();
                    for line_ch in chars.by_ref() {
                        if line_ch == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                _ => {}
            }
        }
        out.push(ch);
    }
    out
}

fn extract_sqlite_api_symbol(declaration: &str) -> Option<String> {
    let declaration = declaration.trim();
    if !declaration.starts_with("SQLITE_API") {
        return None;
    }
    if let Some(open_paren) = declaration.find('(') {
        return last_identifier(&declaration[..open_paren])
            .filter(|symbol| symbol.starts_with("sqlite3_"));
    }
    last_identifier(declaration).filter(|symbol| symbol.starts_with("sqlite3_"))
}

fn last_identifier(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut end = bytes.len();
    while end > 0 && !is_ident_byte(bytes[end - 1]) {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && is_ident_byte(bytes[start - 1]) {
        start -= 1;
    }
    if start == end {
        return None;
    }
    Some(text[start..end].to_owned())
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}
