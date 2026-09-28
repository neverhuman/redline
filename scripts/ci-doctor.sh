#!/usr/bin/env bash
# CI doctor: check the tools a lane needs, PASS/FAIL each, before a long build
# discovers one is missing. Pins come from rust-toolchain.toml and
# ops/ci/lib.sh, the same files CI reads.
#
# Usage:
#   bash scripts/ci-doctor.sh [--profile core|contributor|required]
#
#   core         build and test from source: rustc (rust-toolchain.toml),
#                cargo, a C compiler, pkg-config
#   contributor  core plus just, cargo-nextest, git, jq and curl (`just fast`);
#                rtk is optional
#   required     contributor plus what `just required` (ci-local.sh ->
#                ops/ci/pr-ci.sh) runs: Node 22 and npm, Playwright's
#                Chromium, Docker or REDLINE_TESTING_POSTGRES_URL for the
#                Postgres lane, the pinned jankurai binary, cargo-audit,
#                cargo-deny and gitleaks. Linux x86_64 only: the pinned
#                jankurai and gitleaks builds exist only for it.
#
# The default profile is required (`just ci-doctor`). Exits non-zero when a
# required tool is missing or a pinned version differs.

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
profile=required
case "${1:-}" in
    --profile)
        profile="${2:-}"
        [ $# -eq 2 ] || { printf 'usage: %s [--profile core|contributor|required]\n' "$0" >&2; exit 64; }
        ;;
    '') ;;
    *) printf 'usage: %s [--profile core|contributor|required]\n' "$0" >&2; exit 64 ;;
esac
case "$profile" in
    core | contributor | required) ;;
    *) printf 'ci-doctor: unknown profile %s (core, contributor or required)\n' "$profile" >&2; exit 64 ;;
esac
if [ "$profile" = required ] && [ "$(uname -s)/$(uname -m)" != Linux/x86_64 ]; then
    printf 'ci-doctor: the required profile runs only on Linux x86_64 (this host is %s/%s):\n' "$(uname -s)" "$(uname -m)" >&2
    printf 'the pinned jankurai and gitleaks binaries exist only for it. Use --profile contributor here;\n' >&2
    printf 'CI runs the required lanes for every pull request.\n' >&2
    exit 2
fi

# shellcheck source=ops/ci/lib.sh
. "$ROOT/ops/ci/lib.sh"
set +e

overall=0
emit() { printf '%-4s %-16s %s\n' "$1" "$2" "$3"; }
fail() { overall=1; emit FAIL "$1" "$2"; }
pass() { emit PASS "$1" "$2"; }

check_rust() {
    local expected actual
    expected="$(grep -E '^[[:space:]]*channel' "$ROOT/rust-toolchain.toml" | head -n1 | sed -E 's/.*"([^"]+)".*/\1/')"
    if ! command -v rustc >/dev/null 2>&1; then
        fail rustc "missing (expected ${expected}; install rustup, which reads rust-toolchain.toml)"
        return
    fi
    actual="$(cd "$ROOT" && rustc --version 2>/dev/null | awk '{print $2}')"
    if [ "${actual}" = "${expected}" ]; then
        pass rustc "${actual} (matches rust-toolchain.toml)"
    else
        fail rustc "expected=${expected} actual=${actual}"
    fi
}

check_presence() {
    local tool="$1" hint="$2"
    if command -v "$tool" >/dev/null 2>&1; then
        pass "$tool" "$(command -v "$tool")"
    else
        fail "$tool" "missing (${hint})"
    fi
}

check_c_compiler() {
    local compiler
    for compiler in cc gcc clang; do
        if command -v "$compiler" >/dev/null 2>&1; then
            pass c-compiler "$(command -v "$compiler")"
            return
        fi
    done
    fail c-compiler 'missing (install build-essential, or the Xcode command line tools on macOS)'
}

# check_version <label> <expected> <actual> <install hint>
check_version() {
    if [ -z "$3" ]; then
        fail "$1" "missing (expected $2; install: $4)"
    elif [ "$3" = "$2" ]; then
        pass "$1" "$3"
    else
        fail "$1" "expected=$2 actual=$3 (install: $4)"
    fi
}

check_nextest() {
    local expected=0.9.133
    check_version cargo-nextest "$expected" \
        "$(cargo nextest --version 2>/dev/null | head -n1 | awk '{print $2}')" \
        "cargo install cargo-nextest --locked --version ${expected}"
}

check_optional_rtk() {
    if command -v rtk >/dev/null 2>&1; then
        pass rtk "$(command -v rtk) (optional)"
    else
        pass rtk 'absent (optional: scripts/just/run.sh runs commands directly without it)'
    fi
}

check_node() {
    local actual
    actual="$(node --version 2>/dev/null)"
    case "$actual" in
        v22.*) pass node "$actual" ;;
        '') fail node 'missing (expected 22.x; the web console build and its tests use it)' ;;
        *) fail node "expected=22.x actual=${actual}" ;;
    esac
    check_presence npm 'ships with Node 22'
}

check_chromium() {
    local browsers="${PLAYWRIGHT_BROWSERS_PATH:-$HOME/.cache/ms-playwright}"
    if ls -d "$browsers"/chromium-* >/dev/null 2>&1; then
        pass chromium "Playwright Chromium in ${browsers}"
    else
        fail chromium "no Playwright Chromium in ${browsers} (run: cd subrepos/redline-web/apps/web && npm ci && npx playwright install chromium)"
    fi
}

check_postgres() {
    if [ -n "${REDLINE_TESTING_POSTGRES_URL:-}" ]; then
        pass postgres "REDLINE_TESTING_POSTGRES_URL is set"
    elif docker info >/dev/null 2>&1; then
        pass docker "$(docker version --format '{{.Server.Version}}' 2>/dev/null) (runs the pinned PostgreSQL 16.15 image; see ops/ci/parity.sh)"
    else
        fail docker 'no reachable Docker daemon and no REDLINE_TESTING_POSTGRES_URL (the Postgres corpus lane needs one)'
    fi
}

check_jankurai() {
    local actual
    if [ ! -x "$CI_JANKURAI_BIN" ]; then
        fail jankurai "missing ${CI_JANKURAI_BIN} (install: bash ops/ci/install-github-tools.sh)"
        return
    fi
    actual="$(sha256sum "$CI_JANKURAI_BIN" | awk '{print $1}')"
    if [ "$actual" = "$CI_JANKURAI_SHA256" ]; then
        pass jankurai "${CI_JANKURAI_VERSION} (sha256 ${actual:0:12})"
    else
        fail jankurai "sha256 ${actual} is not the pinned ${CI_JANKURAI_SHA256} (reinstall: bash ops/ci/install-github-tools.sh)"
    fi
}

check_cargo_audit() {
    local expected=0.22.1
    check_version cargo-audit "$expected" \
        "$(cargo audit --version 2>/dev/null | awk '{print $2}')" \
        "cargo install cargo-audit --locked --version ${expected}"
}

check_cargo_deny() {
    check_version cargo-deny "${CI_CARGO_DENY_VERSION}" \
        "$(cargo-deny --version 2>/dev/null | awk '{print $2}')" \
        "cargo install cargo-deny --locked --version ${CI_CARGO_DENY_VERSION}"
}

check_gitleaks() {
    check_version gitleaks "${CI_GITLEAKS_VERSION}" \
        "$(gitleaks version 2>/dev/null | head -n1 | sed -E 's/^v//')" \
        "${CI_GITLEAKS_ASSET_URL}"
}

check_required_dispatch() {
    local required_block expected
    required_block="$(sed -n '/^[[:space:]]*required)/,/^[[:space:]]*;;/p' "$ROOT/scripts/ci-local.sh")"
    # shellcheck disable=SC2016 # the dispatch line is matched literally
    expected='bash "$ROOT/ops/ci/pr-ci.sh"'
    if printf '%s\n' "$required_block" | grep -Fq "$expected"; then
        pass ci-required "scripts/ci-local.sh -> ops/ci/pr-ci.sh"
    else
        fail ci-required "required must dispatch directly to ops/ci/pr-ci.sh"
    fi
}

printf 'ci-doctor: %s profile (pins: rust-toolchain.toml, ops/ci/lib.sh)\n' "$profile"
printf '%-4s %-16s %s\n' STAT TOOL DETAIL
printf '%s\n' '----------------------------------------------------------'
check_rust
check_presence cargo 'installed with rustup'
check_c_compiler
check_presence pkg-config 'install pkg-config'
if [ "$profile" != core ]; then
    check_presence just 'cargo install just --locked, or your package manager'
    check_nextest
    check_presence git 'install git'
    check_presence jq 'install jq'
    check_presence curl 'install curl'
    check_optional_rtk
fi
if [ "$profile" = required ]; then
    check_node
    check_chromium
    check_postgres
    check_jankurai
    check_cargo_audit
    check_cargo_deny
    check_gitleaks
    check_required_dispatch
fi
printf '%s\n' '----------------------------------------------------------'
if [ "${overall}" -eq 0 ]; then
    printf 'ci-doctor: every %s tool is present and pinned\n' "$profile"
else
    printf 'ci-doctor: one or more %s tools are missing or version-mismatched\n' "$profile" >&2
fi
exit "${overall}"
