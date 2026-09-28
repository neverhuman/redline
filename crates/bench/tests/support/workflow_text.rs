//! Line-level readers for the GitHub workflow files, shared by the CI
//! workflow tests (`ci_trust_boundary.rs`, `ci_workflow_routing.rs`). They
//! read the text the way the rules are written; actionlint (security lane)
//! checks the YAML itself.
#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn read(path: &str) -> String {
    let path = repository_root().join(path);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

pub struct Workflow {
    pub file: String,
    pub text: String,
}

pub fn workflows() -> Vec<Workflow> {
    let directory = repository_root().join(".github/workflows");
    let mut files: Vec<PathBuf> = fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()))
        .map(|entry| entry.expect("workflow entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext == "yml" || ext == "yaml")
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no workflows found");
    files
        .into_iter()
        .map(|path| Workflow {
            file: file_name(&path),
            text: fs::read_to_string(&path).expect("read workflow"),
        })
        .collect()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .expect("file name")
        .to_string_lossy()
        .into_owned()
}

pub fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The lines of a top-level key (`on:` or `jobs:`), header excluded.
pub fn top_level_block<'a>(text: &'a str, key: &str) -> Vec<&'a str> {
    let mut lines = text.lines().skip_while(|line| *line != key);
    if lines.next().is_none() {
        return Vec::new();
    }
    lines
        .take_while(|line| line.is_empty() || line.starts_with(' ') || line.starts_with('#'))
        .collect()
}

/// `(job id, job text)` for every job in the workflow.
pub fn jobs(text: &str) -> Vec<(String, String)> {
    let mut jobs: Vec<(String, String)> = Vec::new();
    for line in top_level_block(text, "jobs:") {
        let header = line
            .strip_prefix("  ")
            .filter(|rest| !rest.starts_with(' ') && !rest.starts_with('#'))
            .and_then(|rest| rest.trim_end().strip_suffix(':'))
            .filter(|key| {
                key.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            });
        if let Some(id) = header {
            jobs.push((id.to_string(), String::new()));
        }
        if let Some((_, body)) = jobs.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    jobs
}

/// The text of one job; panics when the workflow has no such job.
pub fn job(text: &str, id: &str) -> String {
    jobs(text)
        .into_iter()
        .find(|(name, _)| name == id)
        .unwrap_or_else(|| panic!("no job {id}"))
        .1
}

/// The text of every step in a job that uses `action` (for example
/// `actions/checkout@`), from its `uses:` line to the next step.
pub fn steps_using(job: &str, action: &str) -> Vec<String> {
    let lines: Vec<&str> = job.lines().collect();
    let mut steps = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.contains(&format!("uses: {action}")) {
            continue;
        }
        let dash = if line.trim_start().starts_with("- ") {
            indent(line)
        } else {
            indent(line).saturating_sub(2)
        };
        let mut step = (*line).to_string();
        for next in &lines[index + 1..] {
            if next.trim().is_empty() {
                continue;
            }
            if indent(next) <= dash {
                break;
            }
            step.push('\n');
            step.push_str(next);
        }
        steps.push(step);
    }
    steps
}

/// The text of every `actions/checkout` step in a job.
pub fn checkout_steps(job: &str) -> Vec<String> {
    steps_using(job, "actions/checkout@")
}

pub fn run_shell_test(script: &str) {
    let output = Command::new("bash")
        .arg(repository_root().join(script))
        .current_dir(repository_root())
        .output()
        .unwrap_or_else(|error| panic!("run {script}: {error}"));
    assert!(
        output.status.success(),
        "{script} failed ({})\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
