//! Shell tests that build a fixture git repository must first clear the
//! variables that locate a repository (GIT_DIR, GIT_WORK_TREE,
//! GIT_INDEX_FILE, ...). A git hook, such as the documented pre-push gate in
//! a linked worktree, exports them, and `git -C <fixture>` does not override
//! them: the fixture's commits, tags and resets then land in the repository
//! being pushed. ops/ci/tests/fixture-git-isolation.sh runs the preflight
//! ones under such a hook environment; this check covers every script.

use std::fs;
use std::path::{Path, PathBuf};

const CLEAR: &str = "git rev-parse --local-env-vars";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn shell_scripts(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            shell_scripts(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "sh") {
            out.push(path);
        }
    }
}

/// Byte offset of the first non-comment line that runs `git ... init`.
fn first_git_init(text: &str) -> Option<usize> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let code = line.trim_start();
        let words: Vec<&str> = code.split_whitespace().collect();
        let runs_git = words
            .first()
            .is_some_and(|word| *word == "git" || word.starts_with("git_"));
        if !code.starts_with('#') && runs_git && words.contains(&"init") {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

#[test]
fn fixture_git_scripts_clear_the_repository_variables_first() {
    let root = repository_root();
    let mut scripts = Vec::new();
    for dir in ["ops", "scripts"] {
        shell_scripts(&root.join(dir), &mut scripts);
    }
    scripts.sort();
    let mut checked = 0;
    let mut missing = Vec::new();
    for script in &scripts {
        let text = fs::read_to_string(script).expect("read script");
        let Some(init) = first_git_init(&text) else {
            continue;
        };
        checked += 1;
        if text.find(CLEAR).is_none_or(|clear| clear >= init) {
            missing.push(
                script
                    .strip_prefix(&root)
                    .unwrap_or(script)
                    .display()
                    .to_string(),
            );
        }
    }
    assert!(
        checked >= 8,
        "found only {checked} fixture scripts; the scan is broken"
    );
    assert!(
        missing.is_empty(),
        "these scripts run `git init` without first clearing the variables \
         `{CLEAR}` lists, so under a git hook they write to the caller's \
         repository: {missing:?}"
    );
}

#[test]
fn preflight_runs_its_fixture_git_tests_under_the_isolation_harness() {
    let fast = fs::read_to_string(repository_root().join("ops/ci/fast.sh")).expect("fast.sh");
    let harness = fast
        .find("bash ops/ci/tests/fixture-git-isolation.sh")
        .expect("preflight runs ops/ci/tests/fixture-git-isolation.sh");
    let rest = &fast[harness..];
    let call = &rest[..rest.find("\n    bash ").unwrap_or(rest.len())];
    for script in [
        "ops/ci/tests/release-authority.sh",
        "scripts/test-launch-claims.sh",
        "scripts/test-release-version.sh",
        "scripts/parity/test-lint-sqlite-parity-ledger.sh",
    ] {
        assert!(
            call.contains(script),
            "the harness call does not run {script}"
        );
    }
}
