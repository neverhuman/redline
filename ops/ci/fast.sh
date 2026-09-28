#!/usr/bin/env bash
# Fast lane dispatcher for the canonical local + CI pre-merge gate.
#
# The `preflight` stage mirrors the shared fast-lane setup checks.
# The `tests` stage runs the test shards in the same order used by the
# local aggregate lane. CI fans the shards out into separate jobs so each
# one stays under the GitHub timeout, while local `just fast` / `ci-local`
# still exercise the same command set end to end.
#
# Usage:
#   CI_FAST_STAGE=preflight bash ops/ci/fast.sh
#   CI_FAST_STAGE=core bash ops/ci/fast.sh
#   CI_FAST_STAGE=kernel-failpoints bash ops/ci/fast.sh
#   CI_FAST_STAGE=qualification bash ops/ci/fast.sh   # SQ-08 JSON, CI only
#   bash ops/ci/fast.sh                # run preflight + all test shards
#
# Every command in this lane is hard-gated by design.

set -euo pipefail

# shellcheck source=ops/ci/lib.sh
. "$(dirname "$0")/lib.sh"

if [ -z "${REDLINEDB_BENCH_GIT_SHA:-}" ]; then
    export REDLINEDB_BENCH_GIT_SHA="$(git rev-parse HEAD)"
fi

if [ "${1:-}" = "sqlite-parity-report-publish-pr" ]; then
    bash ops/ci/sqlite-parity-report.sh publish-pr
    exit 0
fi

run_preflight() {
    # The core profile first, so a missing compiler or the wrong Rust is named
    # here rather than discovered by a failed build.
    bash scripts/ci-doctor.sh --profile core
    bash scripts/test-ci-doctor.sh
    bash scripts/test-docs-quickstart.sh --static
    # The root invocation only reaches the root workspace. Each subrepo is its
    # own cargo workspace, so `cargo fmt --check` here reports clean while the
    # `components (<name>)` jobs run fmt inside the subrepo and fail -- a full
    # CI round trip to learn something rustfmt knew locally.
    cargo fmt --check
    for subrepo_manifest in subrepos/*/Cargo.toml; do
        [ -e "$subrepo_manifest" ] || continue
        cargo fmt --manifest-path "$subrepo_manifest" --all --check
    done
    bash scripts/check_file_sizes.sh
    bash scripts/check-publish-policy.sh
    # The tests that build fixture git repositories, run as a pre-push hook
    # runs them (GIT_DIR exported) so they must also leave the calling
    # repository alone.
    bash ops/ci/tests/fixture-git-isolation.sh \
        ops/ci/tests/release-authority.sh \
        scripts/test-launch-claims.sh \
        scripts/test-release-version.sh \
        scripts/parity/test-lint-sqlite-parity-ledger.sh
    bash ops/ci/tests/main-protection.sh
    bash scripts/check-public-hygiene.sh
    bash scripts/release/test-package-layout.sh
    bash scripts/check-launch-claims.sh
    bash ops/ci/check-workflow-permissions.sh
    bash ops/ci/tests/workflow-permissions.sh
    bash scripts/parity/lint-sqlite-parity-ledger.sh
    cargo build --locked -p redlinedb-cli --bin redlinedb
    # README.md's embedding example (test-docs-quickstart.sh keeps them equal),
    # run as README.md tells readers to, twice: the second run opens the
    # database the first one left.
    cargo build --locked -p redlinedb --example readme
    local readme_run
    for readme_run in first second; do
        test "$(cargo run --locked -q -p redlinedb --example readme)" = hello || {
            printf 'preflight: the README example did not print hello on its %s run\n' "$readme_run" >&2
            return 1
        }
    done
    local smoke_directory smoke_binary
    smoke_directory=$(mktemp -d)
    smoke_binary="${CARGO_TARGET_DIR:-$PWD/target}/debug/redlinedb"
    (cd "$smoke_directory"; test "$("$smoke_binary" -batch :memory: 'SELECT 1;')" = 1)
    rm -rf "$smoke_directory"
    cargo check --workspace --locked
}

run_test_stage() {
    case "$1" in
        core)
            cargo nextest run \
                -p redlinedb \
                -p redlinedb-tokio \
                -p redlinedb-sqlx \
                -p redlinedb-server \
                -p redlinedb-cli \
                -p redlinedb-ffi \
                --locked --no-fail-fast
            ;;
        kernel)
            # nextest runs each test in its own process, and the slow-timeout
            # in .config/nextest.toml names a hung test instead of letting it
            # run into the job timeout.
            cargo nextest run -p redlinedb-kernel --locked
            ;;
        kernel-failpoints)
            # The failpoint-gated kernel tests, whole files and single tests,
            # each built with the features it needs (ops/ci/kernel-failpoint-plan.sh
            # lists the runs). The rest of the kernel suite runs in the kernel
            # stage, without the feature.
            local plan features filter
            plan=$(bash ops/ci/kernel-failpoint-plan.sh) || return 1
            while IFS=$'\t' read -r features filter; do
                cargo nextest run -p redlinedb-kernel --features "$features" \
                    --locked --no-fail-fast -E "$filter" < /dev/null || return 1
            done <<< "$plan"
            # The uncertain-commit tests above the kernel (workplan R7) are
            # gated on their own crate's `failpoints` feature.
            cargo nextest run -p redlinedb-sql --features failpoints --locked --test smoke_misc
            cargo nextest run -p redlinedb --features failpoints --locked --test commit_outcome
            ;;
        sql-unit)
            cargo test -p redlinedb-sql --lib --quiet --locked
            ;;
        sql-contracts)
            cargo test -p redlinedb-sql --test postgres_table_functions --test parity_partial_index --quiet --locked
            cargo test -p redlinedb-sql --test phase11_temp_roots --quiet --locked
            cargo test -p redlinedb-sql --test phase11_veox_queue --quiet --locked
            cargo test -p redlinedb-sql --test phase11_xdoug_compat --quiet --locked
            ;;
        sql-integration)
            # crates/sql/tests holds 87 files / ~1140 tests. Before this stage
            # existed CI ran only `--lib` plus the five named sql-contracts
            # targets, so ~82 files never executed in CI at all. nextest runs
            # the binaries in parallel; a sequential `cargo test` over the same
            # set takes well past the job timeout.
            cargo nextest run -p redlinedb-sql --tests --locked
            ;;
        qualification)
            # SQ-08: the typed-value and C ABI qualification objects, emitted
            # from the test runs themselves (target/qualification/*.json).
            bash scripts/qualification/test-emit-sqlite-qualification.sh
            bash scripts/qualification/emit-sqlite-qualification.sh --out target/qualification
            ;;
        bench)
            cargo test -p redlinedb-bench --quiet --locked
            ;;
        *)
            printf 'unknown fast test stage: %s\n' "$1" >&2
            return 1
            ;;
    esac
}

stage="${CI_FAST_STAGE:-all}"
case "$stage" in
    preflight)
        run_preflight
        ;;
    core|kernel|kernel-failpoints|sql-unit|sql-contracts|sql-integration|bench|qualification)
        run_test_stage "$stage"
        ;;
    tests)
        run_test_stage core
        run_test_stage kernel
        run_test_stage kernel-failpoints
        run_test_stage sql-unit
        run_test_stage sql-contracts
        run_test_stage sql-integration
        run_test_stage bench
        ;;
    all)
        run_preflight
        run_test_stage core
        run_test_stage kernel
        run_test_stage kernel-failpoints
        run_test_stage sql-unit
        run_test_stage sql-contracts
        run_test_stage sql-integration
        run_test_stage bench
        ;;
    *)
        printf 'unknown fast stage: %s\n' "$stage" >&2
        exit 1
        ;;
esac
