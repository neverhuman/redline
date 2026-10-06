//! Shared oracle for the post-release fuzz targets.
//!
//! Parser panics that the engine turns into `sql parser panic` errors are
//! raised again so libFuzzer keeps the input. Semantic disagreements are
//! counted and a bounded sample is appended to `REDLINE_FUZZ_DIVERGENCE_LOG`.
//! They do not stop the run: a wrong answer is a finding, and the campaign
//! still has to keep looking for crashes.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use redlinedb_sql::SqlValue;

const ERROR_CLASSES: &[&str] = &[
    "integer overflow",
    "datatype mismatch",
    "unique constraint",
    "not null constraint",
    "check constraint",
    "foreign key",
    "no such",
    "syntax error",
];

const MAX_EXAMPLES_PER_KIND: usize = 20;

static INPUTS: AtomicU64 = AtomicU64::new(0);
static DIVERGENCES: AtomicU64 = AtomicU64::new(0);
static LAST_FLUSH_MS: AtomicU64 = AtomicU64::new(0);
static LOG: OnceLock<Mutex<LogState>> = OnceLock::new();

struct LogState {
    path: Option<PathBuf>,
    counts: Vec<(&'static str, u64)>,
    examples: Vec<(&'static str, usize)>,
}

pub fn parser_panic(err: &dyn std::fmt::Display) -> bool {
    err.to_string().contains("sql parser panic")
}

pub fn error_class(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    ERROR_CLASSES
        .iter()
        .find(|class| lower.contains(*class))
        .copied()
        .unwrap_or("other")
}

pub fn same_value(left: &SqlValue, right: &SqlValue) -> bool {
    match (left, right) {
        (SqlValue::Null, SqlValue::Null) => true,
        (SqlValue::Integer(left), SqlValue::Integer(right)) => left == right,
        (SqlValue::Real(left), SqlValue::Real(right)) => {
            left.to_bits() == right.to_bits() || (left.is_nan() && right.is_nan())
        }
        (SqlValue::Text(left), SqlValue::Text(right)) => left == right,
        (SqlValue::Blob(left), SqlValue::Blob(right)) => left == right,
        _ => false,
    }
}

pub fn same_rows(left: &[Vec<SqlValue>], right: &[Vec<SqlValue>]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left_row, right_row)| {
            left_row.len() == right_row.len()
                && left_row
                    .iter()
                    .zip(right_row)
                    .all(|(left_value, right_value)| same_value(left_value, right_value))
        })
}

fn canonical_key(value: &SqlValue) -> (u8, Vec<u8>) {
    match value {
        SqlValue::Null => (0, Vec::new()),
        SqlValue::Integer(value) => (1, value.to_be_bytes().to_vec()),
        SqlValue::Real(value) => (2, value.to_bits().to_be_bytes().to_vec()),
        SqlValue::Text(value) => (3, value.as_bytes().to_vec()),
        SqlValue::Blob(value) => (4, value.to_vec()),
    }
}

pub fn sort_rows(rows: &mut [Vec<SqlValue>]) {
    rows.sort_by(|left, right| {
        let left_key: Vec<_> = left.iter().map(canonical_key).collect();
        let right_key: Vec<_> = right.iter().map(canonical_key).collect();
        left_key.cmp(&right_key)
    });
}

pub fn skips_filesystem(sql: &str) -> bool {
    let lower = sql.to_ascii_lowercase();
    // ATTACH and LOAD_EXTENSION stay substring checks, including inside
    // identifiers and string literals. The single-space `vacuum into`
    // substring also stays, so a quoted copy of that spelling is still skipped.
    if lower.contains("attach") || lower.contains("load_extension") || lower.contains("vacuum into")
    {
        return true;
    }
    // A newline, a comment, or a schema name between the keywords does not
    // contain that substring. SQLite still writes the INTO path. Skip any
    // statement whose tokens reach INTO after VACUUM and before `;`. INTO
    // inside a comment or a quoted identifier does not count. This excludes
    // more than the substring: `VACUUM main INTO 'path'` and `VACUUM\nINTO`
    // are skipped too.
    vacuum_statement_reaches_into(sql)
}

fn vacuum_statement_reaches_into(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut index = 0usize;
    let mut saw_vacuum = false;
    while index < bytes.len() {
        match bytes[index] {
            b' ' | b'\t' | b'\n' | b'\r' | 0x0c => index += 1,
            b'-' if index + 1 < bytes.len() && bytes[index + 1] == b'-' => {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if index + 1 < bytes.len() && bytes[index + 1] == b'*' => {
                index += 2;
                while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
                {
                    index += 1;
                }
                if index + 1 < bytes.len() {
                    index += 2;
                } else {
                    break;
                }
            }
            b'\'' | b'"' | b'`' => {
                let quote = bytes[index];
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == quote {
                        if index + 1 < bytes.len() && bytes[index + 1] == quote {
                            index += 2;
                            continue;
                        }
                        index += 1;
                        break;
                    }
                    index += 1;
                }
            }
            b'[' => {
                index += 1;
                while index < bytes.len() && bytes[index] != b']' {
                    index += 1;
                }
                if index < bytes.len() {
                    index += 1;
                }
            }
            b';' => {
                saw_vacuum = false;
                index += 1;
            }
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => {
                let start = index;
                index += 1;
                while index < bytes.len() && is_sql_ident_continue(bytes[index]) {
                    index += 1;
                }
                let word = &sql[start..index];
                if word.eq_ignore_ascii_case("vacuum") {
                    saw_vacuum = true;
                } else if saw_vacuum && word.eq_ignore_ascii_case("into") {
                    return true;
                }
            }
            _ => index += 1,
        }
    }
    false
}

fn is_sql_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

/// `?N` is stored by resizing a vector to `N` (`ParamLayout::push_numbered`).
/// SQLite's default variable cap is 32766. A longer digit run aborts the
/// process under AddressSanitizer, so the campaign records that input and
/// does not prepare it again.
pub fn numbered_parameter_exceeds_sqlite_cap(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'?' {
            index += 1;
            continue;
        }
        index += 1;
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        let digits = index - start;
        if digits > 5 {
            return true;
        }
        if digits > 0 {
            if let Ok(value) = sql[start..index].parse::<u32>() {
                if value > 32_766 {
                    return true;
                }
            }
        }
    }
    false
}

pub fn skips_recursive(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    bytes
        .windows(9)
        .any(|window| window.eq_ignore_ascii_case(b"recursive"))
}

pub fn note_input() {
    let inputs = INPUTS.fetch_add(1, Ordering::Relaxed) + 1;
    if inputs % 5_000 == 0 {
        flush(false);
    }
}

pub fn note_divergence(kind: &'static str, sql: &str, detail: &str) {
    DIVERGENCES.fetch_add(1, Ordering::Relaxed);
    let state = LOG.get_or_init(open_log);
    let mut guard = state.lock().expect("divergence log");
    if let Some(slot) = guard.counts.iter_mut().find(|(name, _)| *name == kind) {
        slot.1 += 1;
    } else {
        guard.counts.push((kind, 1));
    }
    let seen = guard
        .examples
        .iter()
        .find(|(name, _)| *name == kind)
        .map(|(_, count)| *count)
        .unwrap_or(0);
    if seen < MAX_EXAMPLES_PER_KIND {
        if let Some(slot) = guard.examples.iter_mut().find(|(name, _)| *name == kind) {
            slot.1 += 1;
        } else {
            guard.examples.push((kind, 1));
        }
        if let Some(path) = guard.path.clone() {
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                let sql = sql.chars().take(500).collect::<String>();
                let detail = detail.chars().take(500).collect::<String>();
                let _ = writeln!(
                    file,
                    "{{\"kind\":{},\"sql\":{},\"detail\":{}}}",
                    json_string(kind),
                    json_string(&sql),
                    json_string(&detail)
                );
            }
        }
    }
    drop(guard);
    flush(false);
}

fn open_log() -> Mutex<LogState> {
    let path = std::env::var_os("REDLINE_FUZZ_DIVERGENCE_LOG").map(PathBuf::from);
    if let Some(path) = &path {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
    }
    Mutex::new(LogState {
        path,
        counts: Vec::new(),
        examples: Vec::new(),
    })
}

fn flush(force: bool) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let previous = LAST_FLUSH_MS.load(Ordering::Relaxed);
    if !force && now.saturating_sub(previous) < 5_000 {
        return;
    }
    LAST_FLUSH_MS.store(now, Ordering::Relaxed);
    let Some(path) = std::env::var_os("REDLINE_FUZZ_SUMMARY") else {
        return;
    };
    let counts = LOG
        .get()
        .and_then(|state| state.lock().ok())
        .map(|guard| {
            guard
                .counts
                .iter()
                .map(|(kind, count)| format!("\"{kind}\":{count}"))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    let body = format!(
        "{{\"inputs\":{},\"divergences\":{},\"counts\":{{{}}}}}\n",
        INPUTS.load(Ordering::Relaxed),
        DIVERGENCES.load(Ordering::Relaxed),
        counts
    );
    let _ = fs::write(path, body);
}

fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}
