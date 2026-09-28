//! S8-06: code from a fork pull request must not reach the self-hosted
//! runners, their shared caches, or a checkout token it does not need.
//!
//! These checks read the workflow text. The security lane runs actionlint on
//! the same files for syntax. `docs/ci-trust-boundary.md` explains the rules.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// True for a `pull_request` run whose head branch lives in another repository.
const UNTRUSTED_PR: &str = "github.event_name == 'pull_request' && github.event.pull_request.head.repo.full_name != github.repository";

/// The only jobs whose checkout keeps the token, and why.
const CHECKOUTS_THAT_PUSH: &[(&str, &str, &str)] = &[(
    "sqlite-parity-report.yml",
    "publish-pr",
    "ops/ci/sqlite-parity-report.sh",
)];

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &str) -> String {
    let path = repository_root().join(path);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn routed_runs_on() -> String {
    format!(
        "runs-on: ${{{{ ({UNTRUSTED_PR}) && 'ubuntu-24.04' || fromJSON('[\"self-hosted\",\"Linux\",\"X64\"]') }}}}"
    )
}

struct Workflow {
    file: String,
    text: String,
}

fn workflows() -> Vec<Workflow> {
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

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The lines of a top-level key (`on:` or `jobs:`), header excluded.
fn top_level_block<'a>(text: &'a str, key: &str) -> Vec<&'a str> {
    let mut lines = text.lines().skip_while(|line| *line != key);
    if lines.next().is_none() {
        return Vec::new();
    }
    lines
        .take_while(|line| line.is_empty() || line.starts_with(' ') || line.starts_with('#'))
        .collect()
}

/// `(job id, job text)` for every job in the workflow.
fn jobs(text: &str) -> Vec<(String, String)> {
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

/// The text of every `actions/checkout` step in a job.
fn checkout_steps(job: &str) -> Vec<String> {
    let lines: Vec<&str> = job.lines().collect();
    let mut steps = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.contains("uses: actions/checkout@") {
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

/// Workflows a pull request can start: those triggered by `pull_request` and
/// every local workflow they call.
fn pull_request_reachable(workflows: &[Workflow]) -> BTreeSet<String> {
    let mut reachable: BTreeSet<String> = workflows
        .iter()
        .filter(|workflow| {
            top_level_block(&workflow.text, "on:")
                .iter()
                .any(|line| line.trim_start().starts_with("pull_request"))
        })
        .map(|workflow| workflow.file.clone())
        .collect();
    loop {
        let before = reachable.len();
        for workflow in workflows {
            if !reachable.contains(&workflow.file) {
                continue;
            }
            for line in workflow.text.lines() {
                if let Some(called) = line.trim().strip_prefix("uses: ./.github/workflows/") {
                    reachable.insert(called.trim().to_string());
                }
            }
        }
        if reachable.len() == before {
            return reachable;
        }
    }
}

fn run_shell_test(script: &str) {
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

#[test]
fn every_checkout_drops_its_token_unless_the_job_pushes() {
    let mut failures = Vec::new();
    for workflow in workflows() {
        for (job, body) in jobs(&workflow.text) {
            let pushes = CHECKOUTS_THAT_PUSH
                .iter()
                .any(|(file, id, _)| *file == workflow.file && *id == job);
            let wanted = if pushes {
                "persist-credentials: true"
            } else {
                "persist-credentials: false"
            };
            for step in checkout_steps(&body) {
                if !step.contains(wanted) {
                    failures.push(format!(
                        "{} job {job}: checkout lacks `{wanted}`",
                        workflow.file
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));

    for (file, job, script) in CHECKOUTS_THAT_PUSH {
        assert!(
            read(script).contains("git push origin"),
            "{file} job {job} keeps its checkout token only because {script} pushes; it no longer does"
        );
    }
}

#[test]
fn fork_pull_requests_never_select_a_self_hosted_runner() {
    let workflows = workflows();
    let reachable = pull_request_reachable(&workflows);
    assert!(
        reachable.contains("ci.yml") && reachable.contains("packages.yml"),
        "expected ci.yml and the packages.yml it calls to run on pull requests, got {reachable:?}"
    );

    let routed = routed_runs_on();
    let mut failures = Vec::new();
    for workflow in workflows.iter().filter(|w| reachable.contains(&w.file)) {
        for (job, body) in jobs(&workflow.text) {
            let code = body
                .lines()
                .filter(|line| !line.trim_start().starts_with('#'));
            for line in code.filter(|line| line.contains("self-hosted")) {
                if line.trim() != routed {
                    failures.push(format!(
                        "{} job {job}: `{}` can put a fork pull request on a self-hosted runner",
                        workflow.file,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{}\nexpected exactly: {routed}",
        failures.join("\n")
    );
}

#[test]
fn fork_pull_requests_get_a_job_local_cargo_home() {
    let workflows = workflows();
    let reachable = pull_request_reachable(&workflows);
    let routed = routed_runs_on();
    let selector = [
        format!("UNTRUSTED_PR: ${{{{ {UNTRUSTED_PR} }}}}"),
        "if [ \"${UNTRUSTED_PR}\" = true ]; then".to_string(),
        "cargo_home=\"${RUNNER_TEMP}/cargo-home\"".to_string(),
        "cargo_home=\"${RUNNER_TOOL_CACHE}/redlinedb-cargo\"".to_string(),
    ];

    let mut failures = Vec::new();
    for workflow in workflows.iter().filter(|w| reachable.contains(&w.file)) {
        for (job, body) in jobs(&workflow.text) {
            let self_hosted = body.lines().any(|line| line.trim() == routed);
            let uses_shared_cache = body.contains("redlinedb-cargo");
            if !(self_hosted && !checkout_steps(&body).is_empty()) && !uses_shared_cache {
                continue;
            }
            for needle in &selector {
                if !body.contains(needle.as_str()) {
                    failures.push(format!("{} job {job}: missing `{needle}`", workflow.file));
                }
            }
            if body.matches("redlinedb-cargo").count() != 1 {
                failures.push(format!(
                    "{} job {job}: names the shared cargo cache outside the trust selector",
                    workflow.file
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn heavy_lanes_wait_for_a_maintainer_run_on_fork_pull_requests() {
    let ci = read(".github/workflows/ci.yml");
    let jobs = jobs(&ci);
    let job = |id: &str| -> String {
        jobs.iter()
            .find(|(name, _)| name == id)
            .unwrap_or_else(|| panic!("ci.yml has no job {id}"))
            .1
            .clone()
    };

    assert!(
        job("parity").contains(&format!("if: ${{{{ !({UNTRUSTED_PR}) }}}}")),
        "the Postgres parity lane must be skipped for fork pull requests"
    );
    assert!(
        job("tests").contains(&format!(
            "- stage: ${{{{ ({UNTRUSTED_PR}) && 'kernel' || '' }}}}"
        )),
        "the sequential kernel stage must be excluded for fork pull requests"
    );

    let required = job("required");
    assert!(required.contains("name: RedlineDB/required"));
    assert!(required.contains(&format!("FORK_PR: ${{{{ {UNTRUSTED_PR} }}}}")));
    assert!(
        required.contains("all(.value.result == \"success\")"),
        "the aggregate gate must still require every job to succeed"
    );
    let fork_branch: String = required
        .split("if [ \"${FORK_PR}\" = true ]; then")
        .nth(1)
        .expect("required job must fail fork pull requests explicitly")
        .lines()
        .take_while(|line| line.trim() != "fi")
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        fork_branch.contains("maintainer") && fork_branch.contains("docs/ci-trust-boundary.md"),
        "the fork failure must tell the contributor what happens next"
    );
    assert!(
        fork_branch.contains("exit 1"),
        "a fork pull request must not pass RedlineDB/required"
    );
}

#[test]
fn cargo_nextest_is_a_digest_pinned_job_local_install() {
    for workflow in workflows() {
        for forbidden in ["command -v cargo-nextest", "get.nexte.st", "| tar"] {
            assert!(
                !workflow.text.contains(forbidden),
                "{} must not contain `{forbidden}`",
                workflow.file
            );
        }
    }

    let ci = read(".github/workflows/ci.yml");
    let tests = jobs(&ci)
        .into_iter()
        .find(|(id, _)| id == "tests")
        .expect("ci.yml tests job")
        .1;
    assert!(
        tests.contains("bash ops/ci/install-nextest.sh \"${RUNNER_TEMP}/nextest-bin\""),
        "the test shards must install nextest into a job-local directory"
    );

    let installer = read("ops/ci/install-nextest.sh");
    let pinned = installer
        .lines()
        .find_map(|line| line.strip_prefix("archive_sha256="))
        .expect("install-nextest.sh pins archive_sha256");
    assert!(
        pinned.len() == 64 && pinned.chars().all(|c| c.is_ascii_hexdigit()),
        "archive_sha256 must be a SHA-256 digest, got {pinned}"
    );
    assert!(installer.contains("sha256sum -c"));
    assert!(!installer.contains("command -v cargo-nextest"));

    run_shell_test("ops/ci/tests/install-nextest.sh");
}

#[test]
fn self_hosted_hook_refuses_fork_pull_request_jobs() {
    let installer = read("ops/ci/install-github-runner.sh");
    assert!(
        installer.contains("runner-job-started.sh"),
        "install-github-runner.sh must install ops/ci/runner-job-started.sh as the job_started hook"
    );
    run_shell_test("ops/ci/tests/runner-job-started.sh");
}
