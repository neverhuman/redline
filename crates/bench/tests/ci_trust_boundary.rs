//! S8-06: code from a fork pull request must not reach the self-hosted
//! runners, their shared caches, or a checkout token it does not need.
//!
//! These checks read the workflow text. The security lane runs actionlint on
//! the same files for syntax. `docs/ci-trust-boundary.md` explains the rules.

use std::collections::BTreeSet;

#[path = "support/workflow_text.rs"]
mod workflow_text;
use workflow_text::{
    Workflow, checkout_steps, jobs, read, run_shell_test, top_level_block, workflows,
};

/// True for a `pull_request` run whose head branch lives in another repository.
const UNTRUSTED_PR: &str = "github.event_name == 'pull_request' && github.event.pull_request.head.repo.full_name != github.repository";

/// The only jobs whose checkout keeps the token, and why.
const CHECKOUTS_THAT_PUSH: &[(&str, &str, &str)] = &[(
    "sqlite-parity-report.yml",
    "publish-pr",
    "ops/ci/sqlite-parity-report.sh",
)];

fn routed_runs_on() -> String {
    format!(
        "runs-on: ${{{{ ({UNTRUSTED_PR}) && 'ubuntu-24.04' || fromJSON('[\"self-hosted\",\"Linux\",\"X64\"]') }}}}"
    )
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
    // Every job must succeed; the one exception, durability-receipt skipped
    // on a run without a release tag, is checked in ci_durability_receipt.rs.
    assert!(
        required.contains("all(.value.result == \"success\" or ($tag == \"\" and .key == \"durability-receipt\" and .value.result == \"skipped\"))"),
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
