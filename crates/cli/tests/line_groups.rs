//! The shell reads a script the way sqlite3 does: one line group at a time,
//! reporting a failure against the group's first line and going on with the
//! next group unless `-bail` is set. Expected bytes are what the pinned
//! sqlite3 3.53.1 shell prints for the same input (its `^--- error here`
//! context lines aside, which RedlineDB does not print).

use std::io::Write;
use std::process::{Command, Stdio};

use assert_cmd::cargo::cargo_bin;

/// Run `script` on stdin with `-batch` plus `args` against `:memory:`.
fn run(args: &[&str], script: &str) -> (String, String, i32) {
    let mut child = Command::new(cargo_bin("redlinedb-cli"))
        .arg("-batch")
        .args(args)
        .arg(":memory:")
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
    (
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        String::from_utf8(output.stderr).expect("utf8 stderr"),
        output.status.code().unwrap_or(-1),
    )
}

#[test]
fn a_failed_group_is_reported_and_the_script_goes_on() {
    // Case 00124: `.bail off`, a failing statement, then one that runs.
    let (out, err, code) = run(&[], ".bail off\nSELECT bad_column;\nSELECT 1;\n");
    assert_eq!(out, "1\n");
    assert_eq!(err, "Parse error near line 2: no such column: bad_column\n");
    assert_eq!(code, 1);
}

#[test]
fn bail_stops_at_the_first_failure() {
    // Case 10198.
    let (out, err, code) = run(&["-bail"], "select bogus;\nSELECT 1;\n");
    assert_eq!(out, "");
    assert_eq!(err, "Parse error near line 1: no such column: bogus\n");
    assert_eq!(code, 1);
}

#[test]
fn a_group_starts_at_its_first_line_with_sql() {
    let script = "SELECT 1;\n\n-- a note\n  SELECT\n x FROM nope;\nSELECT 5;\n";
    let (out, err, code) = run(&[], script);
    assert_eq!(out, "1\n5\n");
    assert_eq!(err, "Parse error near line 4: no such table: nope\n");
    assert_eq!(code, 1);
}

#[test]
fn a_failure_drops_the_rest_of_its_group_only() {
    let (out, err, code) = run(&[], "SELECT 1; SELEC 2; SELECT 3;\nSELECT 4;\n");
    assert_eq!(out, "1\n4\n");
    assert_eq!(
        err,
        "Parse error near line 1: near \"SELEC\": syntax error\n"
    );
    assert_eq!(code, 1);
}

#[test]
fn a_statement_that_fails_while_running_is_an_error() {
    let (out, err, code) = run(&[], "BEGIN;\nBEGIN;\nSELECT 2;\n");
    assert_eq!(out, "2\n");
    assert_eq!(
        err,
        "Error near line 2: cannot start a transaction within a transaction\n"
    );
    assert_eq!(code, 1);
}

#[test]
fn echo_interleaves_input_with_results() {
    let script = ".echo on\n\nSELECT 1;\n-- c\n  SELECT\n 3;\nSELECT 4; SELECT 5;\n";
    let (out, err, code) = run(&[], script);
    assert_eq!(
        out,
        "\nSELECT 1;\n1\n-- c\nSELECT\n 3;\n3\nSELECT 4; SELECT 5;\n4\n5\n"
    );
    assert_eq!(err, "");
    assert_eq!(code, 0);
}

#[test]
fn a_line_that_starts_the_next_statement_keeps_its_group_open() {
    // The group ends only where the whole buffer ends on a statement
    // boundary, as sqlite3_complete decides.
    let script = "SELECT 1; SELECT\n2;\nBEGIN; CREATE TABLE t(\n a INT\n);\nCOMMIT;\n\
                  SELECT count(*) FROM t;\n";
    let (out, err, code) = run(&[], script);
    assert_eq!(err, "");
    assert_eq!(out, "1\n2\n0\n");
    assert_eq!(code, 0);
}

#[test]
fn a_case_end_inside_a_trigger_body_stays_in_the_trigger() {
    // The body's statements run when the trigger fires, never as top-level
    // SQL while the trigger is being created.
    let script = "CREATE TABLE t(a);\nCREATE TABLE audit(x);\nINSERT INTO audit VALUES(1);\n\
                  CREATE TRIGGER tr AFTER INSERT ON t BEGIN \
                  UPDATE t SET a = CASE WHEN new.a>0 THEN 1 ELSE 0 END WHERE rowid=new.rowid; \
                  DELETE FROM audit; END;\n\
                  SELECT count(*) FROM audit;\nINSERT INTO t VALUES(5);\n\
                  SELECT count(*) FROM audit;\nSELECT a FROM t;\n";
    let (out, err, code) = run(&[], script);
    assert_eq!(err, "");
    assert_eq!(out, "1\n0\n1\n");
    assert_eq!(code, 0);
}

#[test]
fn sql_arguments_stop_at_the_first_failure() {
    // Unlike stdin, SQL given on the command line stops where it fails, as
    // in sqlite3, so a failed statement is not followed by later writes.
    let output = Command::new(cargo_bin("redlinedb-cli"))
        .args([":memory:", "SELECT bad;\nSELECT 2;"])
        .env("REDLINEDB_QUIET_DURABILITY", "1")
        .output()
        .expect("run redlinedb cli");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "Parse error near line 1: no such column: bad\n"
    );
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn a_dollar_quoted_body_keeps_its_group_open() {
    // A PL/pgSQL body spans lines that end in `;`; the group ends only
    // after the closing `$$` (PostgreSQL corpus case 20301).
    let script = "CREATE FUNCTION double_it(n int) RETURNS int LANGUAGE plpgsql AS $$\n\
                  DECLARE r int;\n\
                  BEGIN r := n * 2; RETURN r; END;\n\
                  $$;\n\
                  SELECT double_it(21);\n";
    let mut child = Command::new(cargo_bin("redlinedb-cli"))
        .args(["-batch", "-bail", ":memory:"])
        .env("REDLINEDB_QUIET_DURABILITY", "1")
        .env("REDLINEDB_RESULT_DIALECT", "postgres")
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
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "42\n");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn ifexists_names_the_missing_file() {
    // Case 00194.
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("missing.db");
    let output = Command::new(cargo_bin("redlinedb-cli"))
        .arg("-ifexists")
        .arg(&missing)
        .arg("SELECT 1;")
        .output()
        .expect("run redlinedb cli");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!(
            "Error: unable to open database \"{}\": unable to open database file\n",
            missing.display()
        )
    );
}
