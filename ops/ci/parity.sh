#!/usr/bin/env bash
# Official redline-testing dispatcher with fail-closed legacy local parity
# stage names.
#
# Usage:
#   CI_PARITY_STAGE=redline-testing-official bash ops/ci/parity.sh
#   CI_PARITY_STAGE=all bash ops/ci/parity.sh

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

# GitHub's parity job injects REDLINE_TESTING_POSTGRES_URL from a service
# container. A local `just pr-ci` does not. Without that URL the beyond suite
# records no reference, the gate refuses to write postgres-qualification.json,
# and the official evidence step fails while looking for that file. Reuse a
# listener that already matches the pinned 16.15 identity; do not start one.
use_local_postgres_oracle() {
    if [ -n "${REDLINE_TESTING_POSTGRES_URL:-}" ]; then
        return 0
    fi
    if ! command -v psql >/dev/null 2>&1; then
        printf 'parity: REDLINE_TESTING_POSTGRES_URL is unset and psql is not on PATH\n' >&2
        return 0
    fi
    local port="${REDLINEDB_POSTGRES_PORT:-55432}"
    local url="postgres://redlinedb:postgres@127.0.0.1:${port}/redlinedb_beyond"
    local settings
    settings="$(
        PGPASSWORD=postgres psql "$url" -q -A -t -X -F '|' \
            -c "SELECT current_setting('server_version_num'), datcollate, datctype, current_setting('TimeZone') FROM pg_database WHERE datname = current_database();" \
            2>/dev/null | tr -d '[:space:]' || true
    )"
    if [ "$settings" != "160015|C|C|UTC" ]; then
        printf 'parity: REDLINE_TESTING_POSTGRES_URL is unset and 127.0.0.1:%s is not PostgreSQL 16.15 C/UTC\n' "$port" >&2
        return 0
    fi
    export REDLINE_TESTING_POSTGRES_URL="$url"
    if [ -z "${REDLINE_TESTING_POSTGRES_IMAGE:-}" ]; then
        export REDLINE_TESTING_POSTGRES_IMAGE="sha256:efdf07c2f9d4df592783dcc8ea5f6db02efbf5f6452b527225ff5e58364570e9"
    fi
    printf 'parity: using local PostgreSQL 16.15 oracle on 127.0.0.1:%s\n' "$port" >&2
}

if [ -z "${REDLINEDB_BENCH_GIT_SHA:-}" ]; then
    REDLINEDB_BENCH_GIT_SHA="$(git rev-parse HEAD)"
    export REDLINEDB_BENCH_GIT_SHA
fi

run_just_lane() {
    bash scripts/just/run.sh "$1"
}

reject_local_parity_stage() {
    printf '%s is disabled: SQLite parity coverage, benchmark, report, sentinel, and proof evidence must be produced only through the pinned neverhuman/redline-testing release artifact.\n' "$1" >&2
    return 1
}

run_stage() {
    case "$1" in
        redline-testing-official)
            run_just_lane redline-testing-official
            bash ops/ci/check-report.sh
            ;;
        sql-parity-all-tests)
            reject_local_parity_stage "$1"
            ;;
        sql-parity-full)
            reject_local_parity_stage "$1"
            ;;
        ffi-parity-full)
            reject_local_parity_stage "$1"
            ;;
        cli-parity-full)
            reject_local_parity_stage "$1"
            ;;
        fuzz-parity)
            reject_local_parity_stage "$1"
            ;;
        fuzz-parity-nightly)
            reject_local_parity_stage "$1"
            ;;
        beyond-sqlite-manifest)
            run_just_lane beyond-sqlite-manifest
            ;;
        *)
            printf 'unknown parity stage: %s\n' "$1" >&2
            return 1
            ;;
    esac
}

stage="${CI_PARITY_STAGE:-all}"
use_local_postgres_oracle
case "$stage" in
    all)
        run_stage redline-testing-official
        ;;
    *)
        run_stage "$stage"
        ;;
esac
