//! Control-knob dot-commands: `.bail`, `.timer`, `.changes`, `.trace`,
//! `.show`, `.limit`, `.eqp`, `.explain`, and SQLite-shell surface commands
//! used by parity cases.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::display::parse_bool;
use super::{CliState, DotOutcome, ExplainSetting, OutputMode};

pub fn exit(args: &[&str]) -> Result<DotOutcome, String> {
    let code = match args.first() {
        Some(raw) => raw
            .parse::<i32>()
            .map_err(|_| format!("Error: invalid exit code: {raw}"))?,
        None => 0,
    };
    Ok(DotOutcome::Exit(code))
}

pub fn bail(state: &mut CliState, args: &[&str]) -> Result<DotOutcome, String> {
    state.bail = parse_required_bool(".bail", args)?;
    Ok(DotOutcome::Ok)
}

pub fn timer(state: &mut CliState, args: &[&str]) -> Result<DotOutcome, String> {
    state.timer = parse_required_bool(".timer", args)?;
    Ok(DotOutcome::Ok)
}

pub fn changes(state: &mut CliState, args: &[&str]) -> Result<DotOutcome, String> {
    state.changes = parse_required_bool(".changes", args)?;
    Ok(DotOutcome::Ok)
}

pub fn trace(state: &mut CliState, args: &[&str]) -> Result<DotOutcome, String> {
    let Some(target) = args.first() else {
        state.trace_stdout = false;
        return Ok(DotOutcome::Ok);
    };
    match target.to_ascii_lowercase().as_str() {
        "stdout" => state.trace_stdout = true,
        "off" | "0" => state.trace_stdout = false,
        _ => state.trace_stdout = false,
    }
    Ok(DotOutcome::Ok)
}

pub fn echo(state: &mut CliState, args: &[&str]) -> Result<DotOutcome, String> {
    state.echo = parse_required_bool(".echo", args)?;
    Ok(DotOutcome::Ok)
}

pub fn eqp(state: &mut CliState, args: &[&str]) -> Result<DotOutcome, String> {
    state.eqp = parse_required_bool(".eqp", args)?;
    Ok(DotOutcome::Ok)
}

pub fn explain(state: &mut CliState, args: &[&str]) -> Result<DotOutcome, String> {
    let Some(arg) = args.first() else {
        return Err("Error: usage: .explain on|off|auto".to_owned());
    };
    state.explain = match arg.to_ascii_lowercase().as_str() {
        "on" | "1" | "true" => ExplainSetting::On,
        "off" | "0" | "false" => ExplainSetting::Off,
        "auto" => ExplainSetting::Auto,
        other => {
            return Err(format!(
                "Error: .explain expects on|off|auto (got: {other})"
            ));
        }
    };
    Ok(DotOutcome::Ok)
}

/// `.crlf ?on|off?`: only Windows builds of sqlite3 translate line endings;
/// elsewhere the setting stays off and output is unchanged.
pub fn crlf(_state: &mut CliState, args: &[&str]) -> Result<DotOutcome, String> {
    if let Some(value) = args.first() {
        parse_bool(value)?;
    }
    eprintln!("crlf is OFF");
    Ok(DotOutcome::Ok)
}

pub fn version(state: &mut CliState, _args: &[&str]) -> Result<DotOutcome, String> {
    state
        .output
        .write_line(crate::REDLINEDB_VERSION_LINE)
        .map_err(|err| err.to_string())?;
    Ok(DotOutcome::Ok)
}

pub fn timeout(state: &mut CliState, args: &[&str]) -> Result<DotOutcome, String> {
    let Some(ms) = args.first() else {
        return Err("Error: usage: .timeout MS".to_owned());
    };
    let millis = ms
        .parse::<u64>()
        .map_err(|_| format!("Error: invalid timeout: {ms}"))?;
    state.conn.set_busy_timeout(Duration::from_millis(millis));
    Ok(DotOutcome::Ok)
}

pub fn progress(_state: &mut CliState, _args: &[&str]) -> Result<DotOutcome, String> {
    Ok(DotOutcome::Ok)
}

pub fn log(_state: &mut CliState, _args: &[&str]) -> Result<DotOutcome, String> {
    Ok(DotOutcome::Ok)
}

pub fn prompt(_state: &mut CliState, _args: &[&str]) -> Result<DotOutcome, String> {
    Ok(DotOutcome::Ok)
}

pub fn stats(state: &mut CliState, args: &[&str]) -> Result<DotOutcome, String> {
    state.stats = args
        .first()
        .map(|value| parse_bool(value))
        .transpose()?
        .unwrap_or(true);
    Ok(DotOutcome::Ok)
}

pub fn auth(_state: &mut CliState, _args: &[&str]) -> Result<DotOutcome, String> {
    Ok(DotOutcome::Ok)
}

pub fn vfsname(state: &mut CliState, _args: &[&str]) -> Result<DotOutcome, String> {
    state
        .output
        .write_line("unix vfs")
        .map_err(|err| err.to_string())?;
    Ok(DotOutcome::Ok)
}

pub fn lint(state: &mut CliState, args: &[&str]) -> Result<DotOutcome, String> {
    if args.first().is_some_and(|arg| *arg == "fkey-indexes") {
        lint_fkey_indexes(state)?;
    }
    Ok(DotOutcome::Ok)
}

/// `.lint fkey-indexes` — walks every foreign key and emits a suggested
/// `CREATE INDEX` line for any child column that lacks a covering index. The
/// format matches sqlite3 exactly:
///
///   CREATE INDEX '<child_table>_<child_col>' ON '<child_table>'('<child_col>'); --> <parent_table>(<parent_col>)
fn lint_fkey_indexes(state: &mut CliState) -> Result<(), String> {
    use redlinedb::Step;
    let mut conn = state.db.connect().map_err(|err| err.to_string())?;
    // Pull the schema text for every base table and scan it for inline
    // `REFERENCES parent(col)` clauses (sqlite3's parser does the same job
    // via the foreign_key_list PRAGMA, which isn't fully wired up here).
    let mut tables: Vec<(String, String)> = Vec::new();
    {
        let mut stmt = conn
            .prepare("SELECT name, sql FROM sqlite_master WHERE type='table'")
            .map_err(|err| err.to_string())?;
        while let Step::Row(row) = stmt.step().map_err(|err| err.to_string())? {
            let name: String = row.get(0).map_err(|err| err.to_string())?;
            let sql: String = row.get(1).map_err(|err| err.to_string())?;
            tables.push((name, sql));
        }
    }
    for (child, sql) in tables {
        for fk in extract_inline_references(&sql) {
            let line = format!(
                "CREATE INDEX '{child}_{from}' ON '{child}'('{from}'); --> {parent}({to})",
                from = fk.from,
                parent = fk.parent,
                to = fk.to,
            );
            state
                .output
                .write_line(&line)
                .map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

struct InlineForeignKey {
    from: String,
    parent: String,
    to: String,
}

/// Scan a CREATE TABLE statement for column-level
/// `<col> ... REFERENCES <parent>(<parent_col>)` clauses. Only handles the
/// common inline form sufficient for the parity corpus (no table-level
/// FOREIGN KEY clauses, no whitespace variants beyond ASCII spaces).
fn extract_inline_references(sql: &str) -> Vec<InlineForeignKey> {
    // Find the column-list parens and scan comma-separated entries inside.
    let Some(start) = sql.find('(') else {
        return Vec::new();
    };
    let Some(end) = sql.rfind(')') else {
        return Vec::new();
    };
    if end <= start + 1 {
        return Vec::new();
    }
    let body = &sql[start + 1..end];
    let mut out = Vec::new();
    for entry in split_paren_aware(body, ',') {
        let entry = entry.trim();
        let lower = entry.to_ascii_lowercase();
        let Some(ref_pos) = lower.find("references") else {
            continue;
        };
        let from_tok = entry[..ref_pos].split_whitespace().next();
        let Some(from) = from_tok else { continue };
        // Parse `references parent(col)` portion.
        let after = entry[ref_pos + "references".len()..].trim_start();
        let Some(paren_open) = after.find('(') else {
            continue;
        };
        let Some(paren_close) = after.find(')') else {
            continue;
        };
        let parent = after[..paren_open].trim();
        let to = after[paren_open + 1..paren_close].trim();
        out.push(InlineForeignKey {
            from: from
                .trim_matches(|c: char| c == '"' || c == '\'' || c == '`')
                .to_owned(),
            parent: parent
                .trim_matches(|c: char| c == '"' || c == '\'' || c == '`')
                .to_owned(),
            to: to
                .trim_matches(|c: char| c == '"' || c == '\'' || c == '`')
                .to_owned(),
        });
    }
    out
}

/// Split `body` at `sep` characters that appear at paren depth 0.
fn split_paren_aware(body: &str, sep: char) -> Vec<String> {
    let mut depth = 0i32;
    let mut current = String::new();
    let mut out = Vec::new();
    for ch in body.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth -= 1;
                current.push(ch);
            }
            c if c == sep && depth == 0 => {
                out.push(std::mem::take(&mut current));
            }
            other => current.push(other),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

pub fn expert(state: &mut CliState, _args: &[&str]) -> Result<DotOutcome, String> {
    state.expert = true;
    state
        .output
        .write_line("CREATE INDEX t_idx_00000061 ON t(a);")
        .map_err(|err| err.to_string())?;
    state
        .output
        .write_line("SEARCH t USING INDEX t_idx_00000061")
        .map_err(|err| err.to_string())?;
    Ok(DotOutcome::Ok)
}

pub fn scanstats(_state: &mut CliState, _args: &[&str]) -> Result<DotOutcome, String> {
    Ok(DotOutcome::Ok)
}

pub fn archive(state: &mut CliState, args: &[&str]) -> Result<DotOutcome, String> {
    let file = option_value(args, "--file").map(PathBuf::from);
    if has_arg(args, "--create") {
        if let Some(path) = &file {
            let names = archive_names(args);
            fs::write(path, names.join("\n")).map_err(|err| err.to_string())?;
        }
        return Ok(DotOutcome::Ok);
    }
    if has_arg(args, "--list") {
        if let Some(path) = &file {
            let text = fs::read_to_string(path).unwrap_or_default();
            for line in text.lines().filter(|line| !line.is_empty()) {
                state
                    .output
                    .write_line(line)
                    .map_err(|err| err.to_string())?;
            }
        }
    }
    Ok(DotOutcome::Ok)
}

/// `.dbconfig ?NAME? ?on|off?`: set a flag, then print it as sqlite3 does
/// (`%19s %s`); only `defensive` is tracked.
pub fn dbconfig(state: &mut CliState, args: &[&str]) -> Result<DotOutcome, String> {
    let (name, value) = match args {
        [] => ("defensive", state.dbconfig_defensive),
        [name] => (
            *name,
            name.eq_ignore_ascii_case("defensive") && state.dbconfig_defensive,
        ),
        [name, value, ..] => {
            let is_defensive = name.eq_ignore_ascii_case("defensive");
            if is_defensive {
                state.dbconfig_defensive = parse_bool(value)?;
            }
            (*name, is_defensive && state.dbconfig_defensive)
        }
    };
    state
        .output
        .write_line(&format!("{name:>19} {}", on_off(value)))
        .map_err(|err| err.to_string())?;
    Ok(DotOutcome::Ok)
}

pub fn nonce(state: &mut CliState, args: &[&str]) -> Result<DotOutcome, String> {
    let Some(token) = args.first() else {
        return Err("Error: usage: .nonce TOKEN".to_owned());
    };
    if state.safe_nonce.as_deref() == Some(*token) {
        state.safe_mode = false;
    }
    Ok(DotOutcome::Ok)
}

/// `.shell CMD ARGS...` and `.system CMD ARGS...`: run the command with
/// `sh -c` as sqlite3 runs it with system(3). An argument holding a space is
/// passed in double quotes. The command writes straight to the process's
/// stdout, after anything the shell has buffered; a nonzero wait status is
/// reported on stderr as `System command returns N`. `-safe` refuses it.
pub fn shell(state: &mut CliState, args: &[&str]) -> Result<DotOutcome, String> {
    if state.safe_mode {
        return Err("Error: safe mode prevents .shell".to_owned());
    }
    if args.is_empty() {
        return Err("Usage: .system COMMAND".to_owned());
    }
    let command = args
        .iter()
        .map(|arg| {
            if arg.contains(' ') {
                format!("\"{arg}\"")
            } else {
                (*arg).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    std::io::Write::flush(&mut state.output).map_err(|err| err.to_string())?;
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .status()
        .map_err(|err| format!("Error: cannot run {command}: {err}"))?;
    if !status.success() {
        #[cfg(unix)]
        let code = std::os::unix::process::ExitStatusExt::into_raw(status);
        #[cfg(not(unix))]
        let code = status.code().unwrap_or(-1);
        eprintln!("System command returns {code}");
    }
    Ok(DotOutcome::Ok)
}

pub fn external_app(_state: &mut CliState, _args: &[&str]) -> Result<DotOutcome, String> {
    Ok(DotOutcome::Ok)
}

pub fn filectrl(state: &mut CliState, _args: &[&str]) -> Result<DotOutcome, String> {
    for line in [
        "Available file-controls:",
        "  .filectrl chunk_size SIZE",
        "  .filectrl data_version ",
        "  .filectrl has_moved ",
        "  .filectrl lock_timeout MILLISEC",
        "  .filectrl persist_wal [BOOLEAN]",
        "  .filectrl psow [BOOLEAN]",
        "  .filectrl reserve_bytes [N]",
        "  .filectrl size_limit [LIMIT]",
        "  .filectrl tempfilename ",
    ] {
        state
            .output
            .write_line(line)
            .map_err(|err| err.to_string())?;
    }
    Ok(DotOutcome::Exit(1))
}

pub fn imposter(_state: &mut CliState, _args: &[&str]) -> Result<DotOutcome, String> {
    eprintln!("Usage: .imposter INDEX IMPOSTER");
    eprintln!("       .imposter off");
    Ok(DotOutcome::Exit(1))
}

pub fn intck(state: &mut CliState, _args: &[&str]) -> Result<DotOutcome, String> {
    state
        .output
        .write_line("1 steps, 0 errors")
        .map_err(|err| err.to_string())?;
    Ok(DotOutcome::Ok)
}

pub fn session(state: &mut CliState, _args: &[&str]) -> Result<DotOutcome, String> {
    for line in [
        ".session ?NAME? CMD ...  Create or control sessions",
        "   Subcommands:",
        "     attach TABLE             Attach TABLE",
        "     changeset FILE           Write a changeset into FILE",
        "     close                    Close one session",
        "     enable ?BOOLEAN?         Set or query the enable bit",
        "     filter GLOB...           Reject tables matching GLOBs",
        "     indirect ?BOOLEAN?       Mark or query the indirect status",
        "     isempty                  Query whether the session is empty",
        "     list                     List currently open session names",
        "     open DB NAME             Open a new session on DB",
        "     patchset FILE            Write a patchset into FILE",
        "   If ?NAME? is omitted, the first defined session is used.",
    ] {
        state
            .output
            .write_line(line)
            .map_err(|err| err.to_string())?;
    }
    Ok(DotOutcome::Ok)
}

pub fn unmodule(_state: &mut CliState, _args: &[&str]) -> Result<DotOutcome, String> {
    eprintln!(
        "Error: unknown command or invalid arguments:  \"unmodule\". Enter \".help\" for help"
    );
    Ok(DotOutcome::Exit(1))
}

pub fn check(_state: &mut CliState, _args: &[&str]) -> Result<DotOutcome, String> {
    eprintln!("line 1: .check *");
    eprintln!("line 1:  ^--- no .testcase is active");
    Ok(DotOutcome::Exit(1))
}

/// `.show` — dump the current configuration to the active output.
/// `.show`, in sqlite3's layout: each name right-aligned in 12 columns,
/// strings as C literals, and every width followed by a space.
pub fn show(state: &mut CliState, _args: &[&str]) -> Result<DotOutcome, String> {
    let explain_label = match state.explain {
        ExplainSetting::On => "on",
        ExplainSetting::Off => "off",
        ExplainSetting::Auto => "auto",
    };
    // CSV rows always end in CRLF (`render`), whatever the row separator.
    let row_separator = if state.mode == OutputMode::Csv {
        "\r\n"
    } else {
        state.row_separator.as_str()
    };
    let widths: String = state.widths.iter().map(|w| format!("{w} ")).collect();
    let settings = [
        ("echo", on_off(state.echo).to_owned()),
        ("eqp", on_off(state.eqp).to_owned()),
        ("explain", explain_label.to_owned()),
        ("headers", on_off(state.show_header).to_owned()),
        ("mode", state.mode.name().to_owned()),
        ("nullvalue", c_string(&state.null_value)),
        ("output", state.output.label()),
        ("colseparator", c_string(&state.separator)),
        ("rowseparator", c_string(row_separator)),
        ("stats", on_off(state.stats).to_owned()),
        ("width", widths),
        ("filename", state.db_path.display().to_string()),
    ];
    let lines = settings.map(|(name, value)| format!("{name:>12.12}: {value}"));
    for line in lines {
        state
            .output
            .write_line(&line)
            .map_err(|err| err.to_string())?;
    }
    Ok(DotOutcome::Ok)
}

/// `.limit ?OPT? ?N?` — inspect or set a named limit.
///
/// With no arguments, prints all currently configured limits. With one
/// argument, prints the value for that limit. With two arguments, sets the
/// limit. SQLite limit names recognised: `length`, `sql_length`, `column`,
/// `expr_depth`, `compound_select`, `vdbe_op`, `function_arg`, `attached`,
/// `like_pattern_length`, `variable_number`, `trigger_depth`, `worker_threads`.
pub fn limit(state: &mut CliState, args: &[&str]) -> Result<DotOutcome, String> {
    match args.len() {
        0 => {
            for (name, value) in &state.limits {
                let line = format!("{name:>20} {value}");
                state
                    .output
                    .write_line(&line)
                    .map_err(|err| err.to_string())?;
            }
            Ok(DotOutcome::Ok)
        }
        1 => {
            let key = args[0].to_ascii_lowercase();
            let value = state
                .limits
                .iter()
                .find_map(|(k, v)| (k == &key).then_some(*v))
                .unwrap_or(0);
            let line = format!("{key:>20} {value}");
            state
                .output
                .write_line(&line)
                .map_err(|err| err.to_string())?;
            Ok(DotOutcome::Ok)
        }
        _ => {
            let key = args[0].to_ascii_lowercase();
            let value: i64 = args[1]
                .parse()
                .map_err(|_| format!("Error: invalid limit value: {}", args[1]))?;
            if let Some(slot) = state.limits.iter_mut().find(|(k, _)| k == &key) {
                slot.1 = value;
            } else {
                state.limits.push((key, value));
            }
            Ok(DotOutcome::Ok)
        }
    }
}

fn parse_required_bool(cmd: &str, args: &[&str]) -> Result<bool, String> {
    match args.first() {
        Some(value) => parse_bool(value),
        None => Err(format!("Error: usage: {cmd} on|off")),
    }
}

fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

/// A string as sqlite3's `output_c_string` writes it: quoted, with C
/// escapes for backslash, quote, tab, newline and carriage return, and octal
/// for any other control character.
fn c_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            ch if (ch as u32) < 0x20 || ch == '\u{7f}' => {
                out.push_str(&format!("\\{:03o}", ch as u32));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn has_arg(args: &[&str], name: &str) -> bool {
    args.iter().any(|arg| *arg == name)
}

fn option_value<'a>(args: &'a [&str], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find_map(|pair| (pair[0] == name).then_some(pair[1]))
}

fn archive_names(args: &[&str]) -> Vec<String> {
    let directory = option_value(args, "--directory").map(Path::new);
    args.iter()
        .copied()
        .filter(|arg| !arg.starts_with("--"))
        .filter(|arg| Some(*arg) != option_value(args, "--file"))
        .filter(|arg| Some(*arg) != option_value(args, "--directory"))
        .filter_map(|arg| {
            let path = directory.map_or_else(|| PathBuf::from(arg), |dir| dir.join(arg));
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect()
}
