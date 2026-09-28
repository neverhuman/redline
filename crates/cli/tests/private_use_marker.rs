//! PG-03: the shell used to strip a leading U+E000 from every TEXT value it
//! printed, because `::citext` marks its values with that character. A
//! value the user stored with U+E000 lost its first character. The shell
//! now hides the marker only in the Postgres dialect after
//! `CREATE EXTENSION citext`.

use std::io::Write;
use std::process::{Command, Stdio};

use assert_cmd::cargo::cargo_bin;

fn run(dialect: Option<&str>, extra_args: &[&str], script: &str) -> String {
    let mut cmd = Command::new(cargo_bin("redlinedb-cli"));
    cmd.arg("-batch").arg("-bail");
    cmd.args(extra_args);
    cmd.arg(":memory:");
    match dialect {
        Some(value) => cmd.env("REDLINEDB_RESULT_DIALECT", value),
        None => cmd.env_remove("REDLINEDB_RESULT_DIALECT"),
    };
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn redlinedb cli");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(script.as_bytes())
        .expect("write script");
    let output = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let stderr = String::from_utf8(output.stderr).expect("utf-8 stderr");
    assert!(output.status.success(), "stderr={stderr} stdout={stdout}");
    stdout
}

#[test]
fn sqlite_dialect_prints_a_leading_private_use_character() {
    for args in [&[][..], &["--no-shellzero"][..]] {
        let out = run(
            None,
            args,
            "CREATE TABLE t(x TEXT);\
             INSERT INTO t VALUES(char(57344)||'A');\
             SELECT x FROM t;\
             SELECT char(57344)||'b';\n",
        );
        assert_eq!(out, "\u{E000}A\n\u{E000}b\n", "args {args:?}");
    }
}

#[test]
fn postgres_dialect_prints_it_until_citext_is_enabled() {
    let out = run(Some("postgres"), &[], "SELECT char(57344)||'A';\n");
    assert_eq!(out, "\u{E000}A\n");
    let out = run(
        Some("postgres"),
        &[],
        "CREATE EXTENSION IF NOT EXISTS citext;\
         SELECT 'Hello'::citext;\
         SELECT 'Hello'::citext = 'HELLO'::citext;\n",
    );
    assert_eq!(out, "Hello\nt\n");
}
