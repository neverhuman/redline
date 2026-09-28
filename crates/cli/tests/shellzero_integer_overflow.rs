//! Launch Q5-06: `SELECT (-9223372036854775807-1)/-1` and `% -1` used to
//! abort the CLI with a core dump. ShellZero evaluated `a / b` and `a % b`
//! on i64 directly, and the release profile has `panic = "abort"`. The
//! checked ops now fall through to the engine, which answers the way the
//! pinned SQLite 3.53.1 shell does.

use std::process::Command;

use assert_cmd::cargo::cargo_bin;

fn run(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(cargo_bin("redlinedb-cli"))
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run redlinedb cli");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

/// (SQL, what `sqlite3 :memory: SQL` prints) for the pinned 3.53.1 shell.
const CASES: &[(&str, &str)] = &[
    (
        "SELECT (-9223372036854775807-1)/-1;",
        "9.2233720368547758e+18",
    ),
    ("SELECT (-9223372036854775807-1)%-1;", "0"),
    (
        "SELECT 9223372036854775807+1, typeof(9223372036854775807+1);",
        "9.2233720368547758e+18|real",
    ),
    (
        "SELECT -(-9223372036854775807-1);",
        "9.2233720368547758e+18",
    ),
    ("SELECT 5.5 % 2, 7 % 2.5;", "1.0|1.0"),
    ("SELECT 7 / 0, 7 % 0;", "|"),
];

#[test]
fn min_div_and_rem_minus_one_do_not_crash_any_route() {
    for (sql, want) in CASES {
        // Auto ShellZero (single SQL argument on :memory:), explicit
        // ShellZero, and the engine with ShellZero disabled.
        for flags in [&[][..], &["--shellzero"][..], &["--no-shellzero"][..]] {
            let mut args: Vec<&str> = flags.to_vec();
            args.extend_from_slice(&["-batch", ":memory:", sql]);
            let (out, err, code) = run(&args);
            assert_eq!(code, 0, "{args:?} exited {code}; stderr={err}");
            assert_eq!(
                out.trim_end(),
                *want,
                "{args:?} stdout={out:?} stderr={err}"
            );
        }
    }
}

#[test]
fn abs_of_min_is_an_integer_overflow_error() {
    for flags in [&[][..], &["--no-shellzero"][..]] {
        let mut args: Vec<&str> = flags.to_vec();
        args.extend_from_slice(&["-batch", ":memory:", "SELECT abs(-9223372036854775807-1);"]);
        let (out, err, code) = run(&args);
        assert_eq!(code, 1, "{args:?} stdout={out:?} stderr={err}");
        assert!(err.contains("integer overflow"), "{args:?} stderr={err}");
    }
}
