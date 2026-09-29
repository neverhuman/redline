//! Dot-commands and options print what the pinned sqlite3 3.53.1 shell
//! prints for the same script. Each expected output below is that shell's,
//! for the sqlite_parity case named beside it.

use std::io::Write;
use std::process::{Command, Stdio};

use assert_cmd::cargo::cargo_bin;

struct Run {
    stdout: Vec<u8>,
    stderr: String,
    code: i32,
}

/// Run the shell with `args` and `script` on stdin.
fn run(args: &[&str], script: &str) -> Run {
    let mut child = Command::new(cargo_bin("redlinedb-cli"))
        .args(args)
        .env("REDLINEDB_QUIET_DURABILITY", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn redlinedb cli");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(script.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait redlinedb cli");
    Run {
        stdout: output.stdout,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code().unwrap_or(-1),
    }
}

/// Stdout of `script` in batch mode against `:memory:`; the run must succeed
/// without writing to stderr.
fn batch(script: &str) -> String {
    let result = run(&["-batch", ":memory:"], script);
    assert_eq!(result.code, 0, "stderr: {}", result.stderr);
    assert_eq!(result.stderr, "");
    String::from_utf8(result.stdout).expect("utf8 stdout")
}

#[test]
fn schema_prints_a_tables_indexes_with_it() {
    // 00115
    let out = batch(
        "CREATE TABLE t(a INT);\nCREATE INDEX idx_t_a ON t(a);\n.tables\n.indexes t\n.schema t\n",
    );
    assert_eq!(
        out,
        "t\nidx_t_a\nCREATE TABLE t(a INT);\nCREATE INDEX idx_t_a ON t(a);\n"
    );
}

#[test]
fn dump_turns_foreign_keys_off_and_puts_rows_after_their_table() {
    // 00117
    assert_eq!(
        batch("CREATE TABLE t(x INT);\nINSERT INTO t VALUES(1);\n.dump\n"),
        "PRAGMA foreign_keys=OFF;\nBEGIN TRANSACTION;\nCREATE TABLE t(x INT);\n\
         INSERT INTO t VALUES(1);\nCOMMIT;\n"
    );
    assert_eq!(
        batch(
            "CREATE TABLE b(x INT);\nINSERT INTO b VALUES(1);\nCREATE TABLE a(y TEXT);\n\
             INSERT INTO a VALUES('q');\nCREATE VIEW v AS SELECT * FROM a;\n\
             CREATE INDEX ib ON b(x);\n\
             CREATE TRIGGER tr AFTER INSERT ON a BEGIN SELECT 1; END;\n.dump\n"
        ),
        "PRAGMA foreign_keys=OFF;\nBEGIN TRANSACTION;\nCREATE TABLE b(x INT);\n\
         INSERT INTO b VALUES(1);\nCREATE TABLE a(y TEXT);\nINSERT INTO a VALUES('q');\n\
         CREATE VIEW v AS SELECT * FROM a;\n\
         CREATE TRIGGER tr AFTER INSERT ON a BEGIN SELECT 1; END;\n\
         CREATE INDEX ib ON b(x);\nCOMMIT;\n"
    );
}

#[test]
fn parameter_list_pads_names_and_quotes_values() {
    // 00121
    assert_eq!(
        batch(
            ".mode list\n.parameter init\n.parameter set @x 7\nSELECT @x, typeof(@x);\n.parameter list\n"
        ),
        "7|integer\n@x 7\n"
    );
}

#[test]
fn dbconfig_prints_the_flag_after_setting_it() {
    // 00128
    assert_eq!(
        batch(".dbconfig defensive on\n.dbconfig defensive\n"),
        "          defensive on\n          defensive on\n"
    );
}

#[test]
fn connection_lists_and_switches_slots() {
    // 00129
    assert_eq!(
        batch(".connection\n.connection 1\n.open :memory:\nSELECT 1;\n.connection 0\nSELECT 2;\n"),
        "ACTIVE 0: :memory:\n1\n2\n"
    );
    // Each slot holds its own database.
    assert_eq!(
        batch(
            "CREATE TABLE zero(x);\n.connection 1\nCREATE TABLE one(x);\n.tables\n\
             .connection 0\n.tables\n"
        ),
        "one\nzero\n"
    );
    let closed = run(
        &["-batch", ":memory:"],
        ".connection 1\n.connection close 1\n.print after\n",
    );
    assert_eq!(
        closed.stderr,
        "cannot close the active database connection\n"
    );
    assert_eq!(closed.stdout, b"after\n");
}

#[test]
fn crlf_leaves_line_endings_alone_off_windows() {
    // 00134
    let result = run(
        &["-batch", ":memory:"],
        ".crlf on\nSELECT 1;\n.crlf off\nSELECT 2;\n",
    );
    assert_eq!(result.stdout, b"1\n2\n");
    assert_eq!(result.stderr, "crlf is OFF\ncrlf is OFF\n");
}

#[test]
fn sha3sum_hashes_the_database_content() {
    // 00141
    assert_eq!(
        batch("CREATE TABLE t(x);\nINSERT INTO t VALUES(1);\n.sha3sum\n"),
        "536463bbf637986bfae5f52d889404ad048865c19215ef0c578cf8d3\n"
    );
    assert_eq!(
        batch(
            "CREATE TABLE B(x);\nINSERT INTO B VALUES(2),(3);\nCREATE TABLE a(p,q,r,s);\n\
             INSERT INTO a VALUES('x',1.5,NULL,x'01');\n.sha3sum\n.sha3sum a\n\
             .sha3sum --sha3-256\n"
        ),
        "d90495b15fd87942a87faae1d2537f564dacc50e069d3d86d5e1f437\n\
         ecfebf289c22a708f57428a5d9b181ef1f8e78886a700b496282825a|a\n\
         47e0bd8aa34a4b5ca7d84a031ddb4168edfb5ff5894df8fb11855dfe1849b5bc\n"
    );
}

#[test]
fn clone_reports_each_object_it_copies() {
    // 00152
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("clone.db");
    let script = format!(
        "CREATE TABLE t(x);\nINSERT INTO t VALUES(8);\n.clone {path}\n.open {path}\nSELECT x FROM t;\n",
        path = target.display()
    );
    assert_eq!(batch(&script), "t... done\n8\n");
}

#[test]
fn shell_passes_the_commands_output_through() {
    // 00158, 00159
    assert_eq!(
        batch(".shell printf shell-ok\n.print done\n"),
        "shell-okdone\n"
    );
    assert_eq!(
        batch(".system printf system-ok\n.print done\n"),
        "system-okdone\n"
    );
    let failed = run(&["-batch", ":memory:"], ".shell exit 3\n");
    assert_eq!(failed.stderr, "System command returns 768\n");
    let refused = run(&["-batch", "-safe", ":memory:"], ".shell printf x\n");
    assert!(refused.stderr.contains("safe mode"), "{}", refused.stderr);
    assert_eq!(refused.stdout, b"");
}

#[test]
fn escape_modes_print_control_characters_as_sqlite3_does() {
    // 00222
    let sql = "SELECT char(1)||'x', char(13)||'y';";
    let run_with = |mode: Option<&str>| {
        let mut args = vec!["-batch"];
        if let Some(mode) = mode {
            args.extend(["-escape", mode]);
        }
        args.extend([":memory:", sql]);
        run(&args, "").stdout
    };
    assert_eq!(run_with(None), b"^Ax|^My\n");
    assert_eq!(run_with(Some("ascii")), b"^Ax|^My\n");
    assert_eq!(run_with(Some("symbol")), "\u{2401}x|\u{240d}y\n".as_bytes());
    assert_eq!(run_with(Some("off")), b"\x01x|\ry\n");
}

#[test]
fn csv_and_tabs_quote_by_the_shells_table() {
    let out = batch(".mode csv\nSELECT 'é', 'a b', '', NULL, 'a,b', char(1);\n");
    assert_eq!(out, "\"é\",\"a b\",\"\",,\"a,b\",\"^A\"\r\n");
    let out = batch(".mode tabs\nSELECT 'a b', 'a,b';\n");
    assert_eq!(out, "\"a b\"\ta,b\n");
}

#[test]
fn show_lists_settings_in_sqlite3s_layout() {
    // 10182
    assert_eq!(
        batch(".mode list\n.headers off\n.separator |\n.nullvalue NULL\n.width 3 -5 0\n.show\n"),
        "        echo: off\n         eqp: off\n     explain: auto\n     headers: off\n\
         \x20       mode: list\n   nullvalue: \"NULL\"\n      output: stdout\n\
         colseparator: \"|\"\nrowseparator: \"\\n\"\n       stats: off\n\
         \x20      width: 3 -5 0 \n    filename: :memory:\n"
    );
}

#[test]
fn memtrace_reports_each_allocation() {
    // 10228: the lines report RedlineDB's own allocations.
    let result = run(&["-memtrace", "-list", "-batch", ":memory:"], "SELECT 1;\n");
    assert_eq!(result.code, 0);
    assert_eq!(result.stdout, b"1\n");
    let lines: Vec<&str> = result.stderr.lines().collect();
    assert!(!lines.is_empty());
    for line in &lines {
        let rest = line.strip_prefix("MEMTRACE: ").expect(line);
        assert!(
            rest.starts_with("allocate ")
                || rest.starts_with("free ")
                || rest.starts_with("resize "),
            "{line}"
        );
        assert!(rest.ends_with(" bytes"), "{line}");
    }
    assert!(lines.iter().any(|line| line.starts_with("MEMTRACE: free ")));
    let quiet = run(&["-list", "-batch", ":memory:"], "SELECT 1;\n");
    assert_eq!(quiet.stderr, "");
}
