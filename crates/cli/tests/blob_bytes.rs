//! A BLOB shown as text prints the bytes sqlite3 prints: up to the first
//! NUL, control characters as `^X`, and every other byte as it is, whether
//! or not it is UTF-8 (cases 11306-11362). Expected bytes are the pinned
//! sqlite3 3.53.1 shell's output for the same script.

use std::io::Write;
use std::process::{Command, Stdio};

use assert_cmd::cargo::cargo_bin;

/// Stdout bytes of `script` run with `-batch` against `:memory:`.
fn stdout_bytes(script: &str) -> Vec<u8> {
    let mut child = Command::new(cargo_bin("redlinedb-cli"))
        .args(["-batch", ":memory:"])
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
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn a_blob_prints_its_raw_bytes_in_list_mode() {
    let script = "CREATE TABLE t(c INTEGER);\nINSERT INTO t VALUES (x'01ab');\n\
                  SELECT typeof(c), c FROM t;\n";
    assert_eq!(stdout_bytes(script), b"blob|^A\xab\n");
}

#[test]
fn a_blob_stops_at_its_first_nul() {
    assert_eq!(stdout_bytes("SELECT x'41004243';\n"), b"A\n");
    assert_eq!(stdout_bytes("SELECT x'00';\n"), b"\n");
}

#[test]
fn control_characters_escape_except_line_breaks_and_tabs() {
    assert_eq!(
        stdout_bytes("SELECT x'09410a420d0a430d441b1f7f80ff';\n"),
        b"\tA\nB\r\nC^MD^[^_\x7f\x80\xff\n"
    );
}

#[test]
fn delimited_modes_quote_a_blob_as_sqlite3_does() {
    assert_eq!(
        stdout_bytes(".mode csv\nSELECT x'01ab';\nSELECT x'41';\nSELECT x'';\n"),
        b"\"^A\xab\"\r\nA\r\n\"\"\r\n"
    );
    assert_eq!(
        stdout_bytes(".mode tabs\nSELECT x'01ab';\nSELECT x'22412c42';\n"),
        b"\"^A\xab\"\n\"\"\"A,B\"\n"
    );
}
