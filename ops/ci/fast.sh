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
    bash scripts/parity/lint-sqlite-parity-ledger.sh
    cargo build --locked -p redlinedb-cli --bin redlinedb
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
            # The kernel built with `failpoints`: the lib's own tests plus the
            # test files gated on that feature, which the `kernel` stage
            # compiles to nothing. .config/nextest.toml runs the gated files
            # one test at a time (test group `kernel-failpoints`).
            cargo nextest run -p redlinedb-kernel --features failpoints --locked \
                --lib \
                --test failpoint_smoke \
                --test hnsw_failpoints \
                --test commit_outcome \
                --test recovery_failpoints
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
    core|kernel|kernel-failpoints|sql-unit|sql-contracts|sql-integration|bench)
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
