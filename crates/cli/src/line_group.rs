//! sqlite3's batch input loop (`process_input` in shell.c).
//!
//! Input runs one line group at a time: the lines up to the one that
//! completes a statement. A dot-command is a line of its own and is only
//! recognized between groups; so are the blank and comment-only lines the
//! shell skips. When a group fails, the shell reports the failure against the
//! group's first line (`Parse error near line 2: no such table: t` when the
//! statement could not be prepared, `Error near line 2: ...` when running it
//! failed), drops the rest of that group, and goes on with the next one.
//! `-bail` or `.bail on` stops at the first failure instead. Either way the
//! process exits 1 once anything failed.

use std::io::Write;

use crate::dot::{self, CliState, DotOutcome};

/// What a run returns when `-bail` stopped it. The failure is already on
/// stderr, so callers print nothing more (see [`print_error`]).
pub(crate) const BAILED: &str = "";

/// Print a failure a run returned, unless it was already reported.
pub(crate) fn print_error(message: &str) {
    if message != BAILED {
        eprintln!("{message}");
    }
}

/// Where a statement failed, which is what sqlite3 names in its error line.
#[derive(Debug)]
pub(crate) enum ShellError {
    /// Preparing the statement failed: `Parse error`.
    Prepare(String),
    /// Running the statement failed: `Error`.
    Step(String),
    /// The shell failed around the statement (writing output, `.once`).
    Other(String),
}

impl From<String> for ShellError {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

impl ShellError {
    /// A statement that could not be prepared. A read-only connection
    /// refuses a write when it is prepared; sqlite3 refuses it when it runs,
    /// so that refusal is reported as a run-time error.
    pub(crate) fn prepare(err: &redlinedb::Error) -> Self {
        if err.code() == redlinedb::ErrorCode::ReadOnly {
            return Self::Step(sqlite_message(err));
        }
        Self::Prepare(sqlite_message(err))
    }

    /// A statement that failed while running. RedlineDB resolves some names
    /// and parses some clauses only when the statement first runs; sqlite3
    /// always does that while preparing, so those failures are reported as
    /// parse errors.
    pub(crate) fn step(err: &redlinedb::Error) -> Self {
        let text = sqlite_message(err);
        if resolved_when_prepared(err) {
            Self::Prepare(text)
        } else {
            Self::Step(text)
        }
    }

    /// The line sqlite3 prints for this failure: against the first line of
    /// its group when reading a script, without one at the prompt.
    pub(crate) fn report(&self, line: Option<usize>) -> String {
        let (kind, text) = match self {
            Self::Prepare(text) => ("Parse error", text),
            Self::Step(text) | Self::Other(text) => ("Error", text),
        };
        match line {
            Some(line) => format!("{kind} near line {line}: {text}"),
            None => format!("{kind}: {text}"),
        }
    }
}

/// Category prefixes RedlineDB's errors carry and `sqlite3_errmsg` does not.
/// `unsupported capability:` stays: it is the refusal itself, and the
/// PostgreSQL corpus reads it.
const CATEGORY_PREFIXES: &[&str] = &[
    "kernel error: ",
    "parse error: ",
    "sql parser error: ",
    "unsupported sql: ",
    "bind error: ",
    "transaction state error: ",
    "constraint violation: ",
    "configuration error: ",
];

fn resolved_when_prepared(err: &redlinedb::Error) -> bool {
    err.code() == redlinedb::ErrorCode::NotFound
        || err.message().starts_with("parse error: ")
        || err.message().starts_with("bind error: ")
}

/// An engine error as sqlite3 prints it: the message without RedlineDB's
/// numeric code and category. A write refused by a read-only connection
/// reads as SQLITE_READONLY's message, as `sqlite3_errmsg` gives it.
fn sqlite_message(err: &redlinedb::Error) -> String {
    if err.code() == redlinedb::ErrorCode::ReadOnly {
        return "attempt to write a readonly database".to_owned();
    }
    let mut text = err.message();
    for prefix in CATEGORY_PREFIXES {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest;
        }
    }
    text.replace("unknown column", "no such column")
}

/// Run `input` as sqlite3 runs a script. Returns `Err` when `-bail` stopped
/// the run ([`BAILED`]); every other failure is reported and skipped.
pub(crate) fn run_input(state: &mut CliState, input: &str) -> Result<(), String> {
    let mut group = String::new();
    let mut start = 0;
    for (index, line) in input.lines().enumerate() {
        let trimmed = line.trim();
        if group.is_empty() {
            if is_blank_or_comment(trimmed) {
                echo(state, line);
                continue;
            }
            if trimmed.starts_with('.') {
                run_dot_command(state, line)?;
                continue;
            }
            start = index + 1;
            group.push_str(line.trim_start());
        } else if is_alternate_terminator(trimmed) {
            run_group(state, &group, start)?;
            group.clear();
            continue;
        } else {
            group.push_str(line);
        }
        group.push('\n');
        if trimmed.contains(';') && redlinedb::sql_input_complete(&group) {
            run_group(state, &group, start)?;
            group.clear();
        }
    }
    if !group.trim().is_empty() {
        run_group(state, &group, start)?;
    }
    Ok(())
}

/// A line of `/` or `go` ends the group in front of it.
pub(crate) fn is_alternate_terminator(line: &str) -> bool {
    line == "/" || line.eq_ignore_ascii_case("go")
}

/// A line with nothing but whitespace and complete comments, which the
/// shell skips between groups.
fn is_blank_or_comment(line: &str) -> bool {
    let mut rest = line.trim_start();
    loop {
        if rest.is_empty() || rest.starts_with("--") {
            return true;
        }
        let Some(comment) = rest.strip_prefix("/*") else {
            return false;
        };
        match comment.find("*/") {
            Some(end) => rest = comment[end + 2..].trim_start(),
            None => return false,
        }
    }
}

fn run_group(state: &mut CliState, sql: &str, start: usize) -> Result<(), String> {
    match crate::execute_sql_buffer(state, sql) {
        Ok(()) => Ok(()),
        Err(err) => report(state, &err.report(Some(start))),
    }
}

fn run_dot_command(state: &mut CliState, line: &str) -> Result<(), String> {
    // sqlite3 echoes a dot-command before running it, so a leading
    // `.echo off` is still echoed.
    echo(state, line.trim_end());
    match dot::dispatch(state, line.trim()) {
        Ok(DotOutcome::Ok) => Ok(()),
        Ok(DotOutcome::ReadFile(path)) => match crate::run_script_file(state, &path) {
            Err(message) if message != BAILED => report(state, &message),
            result => result,
        },
        Ok(DotOutcome::Exit(code)) => {
            crate::flush_output_or_exit(state);
            std::process::exit(code);
        }
        Err(message) => report(state, &message),
    }
}

/// With `.echo on`, write an input line to the shell's output, in order
/// with the results of the statements around it.
pub(crate) fn echo(state: &mut CliState, text: &str) {
    if state.echo {
        // A failed write surfaces at the next flush of the same output.
        let _ = writeln!(state.output, "{text}");
    }
}

/// Report a failure; stop the run if `-bail` is on.
fn report(state: &mut CliState, message: &str) -> Result<(), String> {
    state.had_error = true;
    print_error(message);
    if state.bail {
        Err(BAILED.to_owned())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::is_blank_or_comment;

    #[test]
    fn blank_and_comment_only_lines_are_skipped_between_groups() {
        for line in [
            "",
            "   ",
            "-- note",
            "/* a */",
            "/* a */ -- b",
            "/* a */ /* b */",
        ] {
            assert!(is_blank_or_comment(line), "{line:?}");
        }
        for line in ["SELECT 1;", "/* open", "/* a */ SELECT 1;", ".tables"] {
            assert!(!is_blank_or_comment(line), "{line:?}");
        }
    }
}
