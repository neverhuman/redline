//! The generator's capture guard: a shard may declare only fragments the
//! reference shell actually printed, on the stream it printed them.

use super::{GenConfig, RawCase, materialize};
use crate::test_support::{ScratchDir, write_executable};

fn config() -> GenConfig<'static> {
    GenConfig {
        category: "SQL_STRING",
        priority: "P1",
        id_start: 11437,
        required_capabilities: &[],
    }
}

/// A fake reference shell that prints `stdout` and `stderr` and exits
/// `code`.
fn shell(dir: &ScratchDir, name: &str, stdout: &str, stderr: &str, code: i32) -> String {
    let path = dir.path().join(name).join("sqlite3");
    write_executable(
        &path,
        &format!(
            "#!/bin/sh\ncat >/dev/null\nprintf '%s' '{stdout}'\nprintf '%s' '{stderr}' >&2\nexit {code}\n"
        ),
    );
    path.to_str().expect("utf-8 path").to_owned()
}

fn rejection(stdout: Vec<&str>, stderr: Vec<&str>) -> RawCase {
    let mut raw = RawCase::new(
        "STRING_SOUNDEX_ROBERT",
        "soundex",
        "SELECT soundex('Robert');\n",
    );
    raw.compare_stdout = false;
    raw.expect_failure = true;
    raw.expected_stdout_contains = stdout.into_iter().map(str::to_owned).collect();
    raw.expected_stderr_contains = stderr.into_iter().map(str::to_owned).collect();
    raw
}

#[test]
fn a_declared_fragment_the_capture_lacks_is_refused() {
    let dir = ScratchDir::new("xtask-generator-guard");
    let quiet = shell(&dir, "quiet", "", "x\n", 1);
    let error = materialize(
        rejection(vec![], vec!["no such function"]),
        11437,
        &config(),
        &quiet,
    )
    .expect_err("a stderr fragment sqlite3 did not print");
    assert!(
        format!("{error:#}").contains("declares stderr fragment \"no such function\""),
        "{error:#}"
    );

    // A fragment printed on the other stream is still missing.
    let on_stderr = shell(&dir, "stderr", "", "R163\n", 1);
    let error = materialize(
        rejection(vec!["R163"], vec![]),
        11437,
        &config(),
        &on_stderr,
    )
    .expect_err("a stdout fragment printed only on stderr");
    assert!(
        format!("{error:#}").contains("declares stdout fragment \"R163\""),
        "{error:#}"
    );

    // The fragment the shell printed is accepted, with its exit code.
    let rejecting = shell(
        &dir,
        "rejecting",
        "",
        "Error: no such function: soundex\n",
        1,
    );
    let case = materialize(
        rejection(vec![], vec!["no such function"]),
        11437,
        &config(),
        &rejecting,
    )
    .expect("a declared fragment the capture confirms");
    assert_eq!(case.expected_exit, 1);
    assert_eq!(case.expected_stderr_contains, ["no such function"]);
}
