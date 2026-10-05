//! CI-04: `RedlineDB/required` must be reachable while the self-hosted
//! runners' links to github.com and static.rust-lang.org are flaky.
//!
//! - The aggregate and the light jobs run on the self-hosted runners for
//!   trusted events (fork pull requests stay on GitHub-hosted runners), so a
//!   stalled GitHub-hosted queue cannot hold `RedlineDB/required`. Only
//!   `durability-receipt` and native macOS packaging stay hosted for release
//!   tags. Linux ARM64 cross-builds and QEMU archive checks run on X64;
//!   ordinary CI skips native macOS rather than waiting on its hosted queue.
//! - Self-hosted jobs check the pinned toolchain offline
//!   (`ops/ci/ensure-rust.sh`) instead of fetching the channel manifest with
//!   dtolnay/rust-toolchain in every job; tool downloads retry and cache the
//!   verified archive outside the workspace.
//! - A push and a dispatch on one ref no longer cancel each other.
//! - An upload that runs after a failure only warns about missing files.
//!
//! These checks read the workflow text; actionlint (security lane) checks
//! the YAML. `docs/ci-trust-boundary.md` lists where each job runs.

use std::collections::BTreeSet;

#[path = "support/workflow_text.rs"]
mod workflow_text;
use workflow_text::{checkout_steps, job, jobs, read, run_shell_test, steps_using, workflows};

/// Jobs of ci.yml that run on GitHub-hosted runners for every event.
const HOSTED_JOBS: &[&str] = &["durability-receipt"];

/// Light jobs of ci.yml that run on the self-hosted runners for trusted
/// events (ci_trust_boundary.rs pins the exact fork-aware `runs-on`).
const SELF_HOSTED_LIGHT_JOBS: &[&str] = &[
    "required",
    "lint",
    "official-evidence-guard",
    "typecheck",
    "test",
    "components",
    "security",
    "audit",
];

/// Workflows whose self-hosted jobs build Rust and so must run ensure-rust.sh.
const RUST_WORKFLOWS: &[&str] = &["ci.yml", "sqlite-parity-report.yml"];

const RETRYING_CURL: &str = "--retry 5 --retry-all-errors --connect-timeout 20";

fn code_lines(body: &str) -> impl Iterator<Item = &str> {
    body.lines()
        .filter(|line| !line.trim_start().starts_with('#'))
}

fn is_self_hosted(body: &str) -> bool {
    code_lines(body).any(|line| line.contains("self-hosted"))
}

#[test]
fn a_push_and_a_dispatch_on_one_ref_do_not_cancel_each_other() {
    let ci = read(".github/workflows/ci.yml");
    assert!(
        ci.contains("  group: ci-${{ github.event_name }}-${{ github.ref }}\n"),
        "ci.yml's concurrency group must include the event"
    );
    assert!(!ci.contains("group: ci-${{ github.ref }}"));
    let packages = read(".github/workflows/packages.yml");
    assert!(
        packages.contains("  group: packages-${{ github.event_name }}-${{ github.ref }}\n"),
        "packages.yml's concurrency group must include the event"
    );
    let cross = read(".github/workflows/packages-cross.yml");
    assert!(
        cross.contains("  group: packages-cross-${{ github.event_name }}-${{ github.ref }}\n"),
        "packages-cross.yml's concurrency group must include the event"
    );
}

#[test]
fn the_aggregate_and_the_light_jobs_run_on_self_hosted_runners() {
    let ci = read(".github/workflows/ci.yml");
    for id in SELF_HOSTED_LIGHT_JOBS {
        let body = job(&ci, id);
        assert!(
            is_self_hosted(&body),
            "ci.yml job {id} must run on the self-hosted runners for trusted events"
        );
        assert!(
            !body
                .lines()
                .any(|line| line.trim() == "runs-on: ubuntu-24.04"),
            "ci.yml job {id} must not be pinned to a GitHub-hosted runner"
        );
    }
    for id in HOSTED_JOBS {
        let body = job(&ci, id);
        assert!(
            body.lines()
                .any(|line| line.trim() == "runs-on: ubuntu-24.04"),
            "ci.yml job {id} must run on ubuntu-24.04"
        );
        assert!(
            !is_self_hosted(&body),
            "ci.yml job {id} names a self-hosted runner"
        );
        assert!(
            !body.contains("redlinedb-cargo"),
            "ci.yml job {id} uses the self-hosted cargo cache"
        );
    }
    let required = job(&ci, "required");
    assert!(required.contains("    name: RedlineDB/required\n"));
    assert!(
        job(&ci, "official-evidence-guard").contains("bash ops/ci/apt-install.sh ripgrep"),
        "scripts/guard-official-evidence.sh needs rg, which the hosted image lacks"
    );
}

#[test]
fn linux_x86_64_packaging_runs_on_self_hosted_runners_for_every_event() {
    // packages.yml is Linux x86_64 only, on the self-hosted runners for every
    // event, so RedlineDB/required never waits on GitHub-hosted capacity.
    let packages = read(".github/workflows/packages.yml");
    for id in ["build-linux", "runtime-linux"] {
        let body = job(&packages, id);
        assert!(
            !body.contains("    if: "),
            "packages.yml job {id} must run for every event"
        );
        assert!(
            is_self_hosted(&body),
            "packages.yml job {id} must use the self-hosted runners"
        );
    }
    assert!(job(&packages, "build-linux").contains("name: packages-linux-x86_64\n"));
    assert!(job(&packages, "runtime-linux").contains("needs: build-linux\n"));
    let ids: Vec<String> = jobs(&packages).into_iter().map(|(id, _)| id).collect();
    for hosted in ["build", "runtime"] {
        assert!(
            !ids.iter().any(|id| id == hosted),
            "packages.yml still has the hosted {hosted} job; it belongs in packages-cross.yml"
        );
    }
}

#[test]
fn arm64_packaging_uses_self_hosted_cross_build_and_macos_is_release_only() {
    // An optional root runs this acceptance against parent workflow fixtures
    // without moving HEAD or mutating the canonical source checkout.
    let workflow = |path: &str| match std::env::var_os("CI_PACKAGING_WORKFLOW_ROOT") {
        Some(root) => std::fs::read_to_string(std::path::PathBuf::from(root).join(path))
            .expect("packaging workflow fixture"),
        None => read(path),
    };
    let cross = workflow(".github/workflows/packages-cross.yml");
    assert!(
        !cross.contains("ubuntu-22.04-arm"),
        "ARM64 packaging must not wait on a GitHub-hosted ARM64 runner"
    );
    for id in ["build-arm64", "runtime-arm64"] {
        let body = job(&cross, id);
        assert!(
            is_self_hosted(&body),
            "packages-cross.yml job {id} must run on self-hosted X64"
        );
        assert!(
            body.contains("bash ops/ci/arm64-packages.sh"),
            "packages-cross.yml job {id} must build or exercise real ARM64 artifacts"
        );
    }
    for id in ["build-macos", "runtime-macos"] {
        let body = job(&cross, id);
        assert!(!is_self_hosted(&body));
        assert!(
            body.contains("    if: inputs.tag != ''\n"),
            "native macOS must skip ordinary CI, while remaining mandatory on release tags"
        );
        assert!(body.contains("macos-15-intel") && body.contains("macos-15"));
        assert!(
            !body.contains("continue-on-error"),
            "release checks must fail closed"
        );
    }
    let ci = workflow(".github/workflows/ci.yml");
    let packaging_cross = job(&ci, "packaging-cross");
    assert!(packaging_cross.contains("uses: ./.github/workflows/packages-cross.yml\n"));
    assert!(
        packaging_cross
            .contains("github.event.pull_request.head.repo.full_name == github.repository")
    );
    let required = job(&ci, "required");
    assert!(
        required
            .lines()
            .any(|line| line.trim().starts_with("needs:") && line.contains("packaging-cross"))
    );
    let package = read("scripts/package-release.sh");
    for needle in [
        "export CARGO_BUILD_TARGET=$package_target",
        "artifact_dir=$CARGO_TARGET_DIR${package_target:+/$package_target}",
        "host=${package_target:-",
    ] {
        assert!(
            package.contains(needle),
            "cross-packaging must select target artifacts and the target dependency graph: {needle}"
        );
    }
    let dockerfile = read("ops/ci/arm64-packages.Dockerfile");
    assert!(dockerfile.contains("FROM --platform=$BUILDPLATFORM rust:"));
    assert!(dockerfile.contains("REDLINE_PACKAGE_TARGET=aarch64-unknown-linux-gnu"));
    assert!(dockerfile.contains("COPY --from=packages / /workspace/target/packages/"));
    let mirror = read("ops/ci/pr-ci.sh");
    assert!(mirror.contains("timeout 5400 bash ops/ci/arm64-packages.sh build"));
    assert!(mirror.contains("timeout 2700 bash ops/ci/arm64-packages.sh runtime"));
    for command in [
        "test-package-licenses.sh",
        "test-package-ffi.sh",
        "packages.sh installer",
        "packages.sh runtime",
        "packages.sh native-install",
        "packages.sh quickstart",
        "packages.sh published-check",
    ] {
        assert!(
            dockerfile.contains(command),
            "ARM64 archive gate missing {command}"
        );
    }
    run_shell_test("ops/ci/tests/arm64-packages.sh");
}

#[test]
fn required_waits_for_every_merge_gate_job() {
    let ci = read(".github/workflows/ci.yml");
    let required = job(&ci, "required");
    let needs: BTreeSet<String> = required
        .lines()
        .find_map(|line| line.trim().strip_prefix("needs: ["))
        .and_then(|rest| rest.strip_suffix(']'))
        .expect("required lists its needs on one line")
        .split(',')
        .map(|id| id.trim().to_string())
        .collect();
    let others: BTreeSet<String> = jobs(&ci)
        .into_iter()
        .map(|(id, _)| id)
        .filter(|id| id != "required")
        .collect();
    assert_eq!(
        needs, others,
        "RedlineDB/required must need every merge-gate ci.yml job"
    );
    assert!(
        jobs(&ci).into_iter().any(|(id, _)| id == "packaging-cross"),
        "ci.yml must define packaging-cross"
    );
}

#[test]
fn the_cli_release_build_is_not_run_by_two_extra_jobs() {
    // packaging (build-from-source.sh --all) builds the release CLI on every
    // platform; the build and cli jobs both repeated it on Linux.
    let ci = read(".github/workflows/ci.yml");
    let ids: Vec<String> = jobs(&ci).into_iter().map(|(id, _)| id).collect();
    for dropped in ["build", "cli"] {
        assert!(
            !ids.iter().any(|id| id == dropped),
            "ci.yml still has the duplicate {dropped} job"
        );
    }
    assert!(!ci.contains("cargo build --locked --release -p redlinedb-cli"));
}

#[test]
fn self_hosted_jobs_check_the_toolchain_offline() {
    let mut failures = Vec::new();
    for workflow in workflows() {
        for (id, body) in jobs(&workflow.text) {
            if !is_self_hosted(&body) {
                continue;
            }
            if code_lines(&body).any(|line| line.contains("uses: dtolnay/rust-toolchain@")) {
                failures.push(format!(
                    "{} job {id}: dtolnay/rust-toolchain on a self-hosted runner",
                    workflow.file
                ));
            }
            if !RUST_WORKFLOWS.contains(&workflow.file.as_str()) || checkout_steps(&body).is_empty()
            {
                continue;
            }
            let checkout = body.find("uses: actions/checkout@").expect("checkout");
            match body.find("run: bash ops/ci/ensure-rust.sh") {
                Some(ensure) if ensure > checkout => {}
                _ => failures.push(format!(
                    "{} job {id}: no `bash ops/ci/ensure-rust.sh` after the checkout",
                    workflow.file
                )),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    run_shell_test("ops/ci/tests/ensure-rust.sh");
}

#[test]
fn tool_downloads_retry_and_outlive_the_checkout_clean() {
    for script in [
        "ops/ci/install-github-tools.sh",
        "ops/ci/install-nextest.sh",
    ] {
        assert!(
            read(script).contains(RETRYING_CURL),
            "{script} must download with `{RETRYING_CURL}`"
        );
    }
    let tools = read("ops/ci/install-github-tools.sh");
    assert!(
        tools.contains("$RUNNER_TOOL_CACHE/redlinedb-tools"),
        "the jankurai archive must be cached outside target/, which the checkout cleans"
    );
    run_shell_test("ops/ci/tests/install-github-tools.sh");
}

#[test]
fn failure_path_uploads_only_warn() {
    let mut failures = Vec::new();
    for workflow in workflows() {
        for (id, body) in jobs(&workflow.text) {
            for step in steps_using(&body, "actions/upload-artifact@") {
                if step.contains("if: always()") && !step.contains("if-no-files-found: warn") {
                    failures.push(format!(
                        "{} job {id}: an `if: always()` upload without `if-no-files-found: warn`",
                        workflow.file
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn kernel_failpoint_tests_have_their_own_shard() {
    let ci = read(".github/workflows/ci.yml");
    let tests = job(&ci, "tests");
    assert!(tests.contains("          - kernel-failpoints\n"));
    assert!(
        tests.contains(
            "    timeout-minutes: ${{ matrix.stage == 'kernel-failpoints' && 30 || 45 }}\n"
        )
    );
    let fast = read("ops/ci/fast.sh");
    assert!(fast.contains("        kernel-failpoints)\n"));
    // The runs themselves come from ops/ci/kernel-failpoint-plan.sh
    // (checked in ci_kernel_failpoints.rs).
    assert!(fast.contains("plan=$(bash ops/ci/kernel-failpoint-plan.sh)"));
    assert!(fast.contains("cargo nextest run -p redlinedb-kernel --features \"$features\""));
    assert!(fast.contains("core|kernel|kernel-failpoints|"));
}

#[test]
fn the_report_bot_runs_only_when_a_maintainer_dispatches_it() {
    // A scheduled run measured the corpus with `--workers auto` on a shared
    // self-hosted runner after merge and rewrote the README's generated
    // blocks from that measurement. docs/release.md says how the report is
    // regenerated instead.
    let report = read(".github/workflows/sqlite-parity-report.yml");
    let on = workflow_text::top_level_block(&report, "on:");
    assert!(
        on.iter().any(|line| line.trim() == "workflow_dispatch:"),
        "the report workflow must stay dispatchable"
    );
    assert!(
        !on.iter()
            .any(|line| line.trim_start().starts_with("schedule") || line.contains("cron:")),
        "sqlite-parity-report.yml must not run on a schedule"
    );
    let release = read("docs/release.md");
    assert!(release.contains("## Parity report"));
    assert!(release.contains("gh workflow run sqlite-parity-report.yml"));
}
