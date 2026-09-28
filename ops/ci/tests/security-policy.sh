#!/usr/bin/env bash
# Negative tests for the supply-chain and secret-scan policy (S8-08). Each case
# builds an input the policy must refuse, from the fixtures in
# ops/ci/tests/fixtures/security/, and checks that it is refused. Each case has
# a control that must pass, so a refusal for an unrelated reason (a missing
# tool, a broken fixture) is not mistaken for the policy working.
#
#   missing-deny      A Cargo.lock with no deny.toml beside it and no exemption
#                     fails security_component_configs. Controls: the same
#                     lockfile with a deny.toml, and with an exemption.
#   weak-policy       A deny.toml beside the lockfile that does not deny
#                     yanked crates, unknown registries, unknown git sources
#                     and wildcards (here: unknown-git = "warn") fails too.
#   unknown-git       A crate depending on a git repository that allow-git
#                     does not list fails `cargo deny check sources` under the
#                     root deny.toml. Control: the same crate with a path
#                     dependency passes `check sources bans`.
#   wildcard          A publishable crate with a versionless path dependency
#                     fails `cargo deny check bans`. Control: the private
#                     crate above.
#   allowlisted-line  A synthetic GitHub token on the same line as an
#                     allowlisted key_sha256 value is reported by gitleaks with
#                     the repository .gitleaks.toml. Control: the line without
#                     the token has no finding.
#   components        The repository's own component list excludes these
#                     fixtures.
#
# Scratch space is target/security-tests/ under the repository root; the git
# dependency is fetched with a private CARGO_HOME there.
#
#   ops/ci/tests/security-policy.sh [repository-root]
#
# GITLEAKS names the gitleaks binary (default: gitleaks on PATH, else
# target/ci/tools/gitleaks).
set -euo pipefail
# Point every git command at this script's fixtures, never at the caller's
# repository: a git hook (pre-push from a linked worktree) exports GIT_DIR,
# GIT_WORK_TREE and GIT_INDEX_FILE, and `git -C` does not override them.
while read -r variable; do unset "$variable"; done < <(git rev-parse --local-env-vars)
root=$(cd "${1:-$(dirname "${BASH_SOURCE[0]}")/../../..}" && pwd)
fixtures=$root/ops/ci/tests/fixtures/security
# shellcheck source=ops/ci/security-lib.sh
source "$root/ops/ci/security-lib.sh"

gitleaks_bin=${GITLEAKS:-}
if [[ -z $gitleaks_bin ]]; then
    if command -v gitleaks >/dev/null; then
        gitleaks_bin=gitleaks
    else
        gitleaks_bin=$root/target/ci/tools/gitleaks
    fi
fi
command -v "$gitleaks_bin" >/dev/null || { printf 'FAIL: gitleaks not found (%s)\n' "$gitleaks_bin" >&2; exit 1; }
cargo deny --version >/dev/null || { printf 'FAIL: cargo-deny is not installed\n' >&2; exit 1; }

work=$root/target/security-tests
rm -rf "$work"
mkdir -p "$work"
log=$work/log
failures=0

expect() {
    local want=$1 name=$2 rc=0
    shift 2
    "$@" >>"$log" 2>&1 || rc=$?
    if [[ $want == refuse && $rc -ne 0 ]] || [[ $want == pass && $rc -eq 0 ]]; then
        printf 'ok    %-44s (%s, exit %d)\n' "$name" "$want" "$rc"
    else
        printf 'FAIL  %-44s (expected %s, exit %d; see %s)\n' "$name" "$want" "$rc" "$log"
        failures=$((failures + 1))
    fi
}

git_repo() {
    git -C "$1" init -q
    git -C "$1" add -A
    git -C "$1" -c user.name=fixture -c user.email=fixture@example.invalid \
        -c commit.gpgsign=false commit -qm fixture
}

# ---- missing-deny -------------------------------------------------------
checkout=$work/missing-deny
mkdir -p "$checkout"
cp -R "$fixtures/missing-deny/." "$checkout/"
git_repo "$checkout"
expect refuse "missing-deny: lockfile without deny.toml" security_component_configs "$checkout"
cp "$root/deny.toml" "$checkout/tool/deny.toml"
expect pass "missing-deny control: deny.toml beside it" security_component_configs "$checkout"
sed 's/^unknown-git = "deny"/unknown-git = "warn"/' "$root/deny.toml" >"$checkout/tool/deny.toml"
expect refuse "weak-policy: deny.toml with unknown-git = warn" security_component_configs "$checkout"
rm "$checkout/tool/deny.toml"
mkdir -p "$checkout/ops/ci"
printf 'tool\tfixture: exempt on purpose\n' >"$checkout/ops/ci/security-exemptions.tsv"
expect pass "missing-deny control: explicit exemption" security_component_configs "$checkout"

# ---- unknown-git and wildcard ------------------------------------------
dep=$work/dep
mkdir -p "$dep/src"
cp "$fixtures/crates/dep/Cargo.toml.in" "$dep/Cargo.toml"
cp "$fixtures/crates/dep/lib.rs.in" "$dep/src/lib.rs"
git_repo "$dep"

make_app() { # directory, publish, dependency line
    mkdir -p "$1/src"
    sed -e "s|@PUBLISH@|$2|" -e "s|@DEPENDENCY@|$3|" \
        "$fixtures/crates/app/Cargo.toml.in" >"$1/Cargo.toml"
    cp "$fixtures/crates/app/lib.rs.in" "$1/src/lib.rs"
    CARGO_HOME=$work/cargo-home cargo generate-lockfile --manifest-path "$1/Cargo.toml" >>"$log" 2>&1
}
deny() { # manifest directory, checks...
    local manifest=$1/Cargo.toml
    shift
    CARGO_HOME=$work/cargo-home cargo deny --manifest-path "$manifest" \
        check --config "$root/deny.toml" "$@"
}
make_app "$work/app-git" false "fixture-dep = { git = \"file://$dep\" }"
make_app "$work/app-path" false 'fixture-dep = { path = "../dep" }'
make_app "$work/app-public" true 'fixture-dep = { path = "../dep" }'
expect refuse "unknown-git: git source not in allow-git" deny "$work/app-git" sources
expect pass "unknown-git control: path dependency" deny "$work/app-path" sources bans
expect refuse "wildcard: publishable crate, versionless path" deny "$work/app-public" bans

# ---- allowlisted-line ---------------------------------------------------
# A token shaped like a GitHub PAT, built here so no committed file holds one.
token=ghp_$(printf 'redline security policy fixture' | sha256sum | cut -c1-36)
mkdir -p "$work/leak" "$work/clean"
sed "s|@TOKEN@|$token|" "$fixtures/allowlisted-line/evidence.json.in" >"$work/leak/evidence.json"
sed "s|@TOKEN@|none|" "$fixtures/allowlisted-line/evidence.json.in" >"$work/clean/evidence.json"
scan() { # directory, report
    "$gitleaks_bin" dir "$1" --config "$root/.gitleaks.toml" --redact --no-banner \
        --exit-code 1 --report-format json --report-path "$2"
}
expect refuse "allowlisted-line: token beside key_sha256" scan "$work/leak" "$work/leak.json"
expect pass "allowlisted-line control: key_sha256 alone" scan "$work/clean" "$work/clean.json"
if [[ -s $work/leak.json ]] && ! jq -e 'any(.[]; .RuleID == "github-pat")' "$work/leak.json" >/dev/null; then
    printf 'FAIL  allowlisted-line: the finding is not the synthetic token\n'
    failures=$((failures + 1))
fi

# ---- components ---------------------------------------------------------
components_exclude_fixtures() {
    # Capture first: under pipefail, grep -q closing the pipe early would
    # make the producer's SIGPIPE status the result, and `!` would pass it.
    local list
    list=$(security_components "$root") || return 1
    [[ -n $list ]] || return 1
    ! grep -q '^ops/ci/tests/' <<<"$list"
}
expect pass "components: repository list skips fixtures" components_exclude_fixtures

if [[ $failures -ne 0 ]]; then
    printf '%d security policy test(s) failed\n' "$failures" >&2
    exit 1
fi
printf 'Security policy tests passed.\n'
