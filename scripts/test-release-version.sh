#!/usr/bin/env bash
# Tests for ops/ci/release-version.sh (DX-05): which tags may be released,
# `bump`, and the dev tag untagged CI builds package under. Fixture
# repositories stand in for the checkout and a stand-in gh for GitHub, so no
# network is used. Needs git, jq and cargo (only `cargo metadata` and
# `cargo update` on fixture workspaces without dependencies).
#
# Usage: bash scripts/test-release-version.sh
set -euo pipefail
# Point every git command at this script's fixtures, never at the caller's
# repository: a git hook (pre-push from a linked worktree) exports GIT_DIR,
# GIT_WORK_TREE and GIT_INDEX_FILE, and `git -C` does not override them.
while read -r variable; do unset "$variable"; done < <(git rev-parse --local-env-vars)
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
script=$root/ops/ci/release-version.sh
# shellcheck source=ops/release/authority.env
. "$root/ops/release/authority.env"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }
legacy_id=1240106851

# gh: `api repos/<slug> --jq .id` prints MOCK_REPO_ID, or the legacy id for
# the old repository name; `release view <tag>` succeeds for a tag listed in
# MOCK_RELEASES, fails with MOCK_GH_ERROR when set, else "release not found".
mkdir -p "$work/bin"
cat > "$work/bin/gh" <<'GH'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$MOCK_GH_LOG"
case "$1 $2" in
  'api repos/'*)
    case "$2" in
      repos/neverhuman/redline) printf '%s\n' "${MOCK_REPO_ID:?}" ;;
      *) printf '%s\n' 1240106851 ;;
    esac ;;
  'release view')
    if [[ -n ${MOCK_GH_ERROR:-} ]]; then printf '%s\n' "$MOCK_GH_ERROR" >&2; exit 1; fi
    for tag in ${MOCK_RELEASES:-}; do
      [[ $tag != "$3" ]] || { printf '{"tagName":"%s"}\n' "$3"; exit 0; }
    done
    printf 'release not found\n' >&2; exit 1 ;;
  *) printf 'gh stand-in: unexpected call: %s\n' "$*" >&2; exit 2 ;;
esac
GH
chmod +x "$work/bin/gh"
export PATH="$work/bin:$PATH" MOCK_GH_LOG="$work/gh.log" MOCK_REPO_ID=$REDLINE_REPO_ID
export GIT_AUTHOR_NAME=fixture GIT_AUTHOR_EMAIL=fixture@example.invalid
export GIT_COMMITTER_NAME=fixture GIT_COMMITTER_EMAIL=fixture@example.invalid
git_() { git -c init.defaultBranch=main -c tag.gpgSign=false -c commit.gpgSign=false "$@"; }

# workspace <dir> <version of a> <version of b> <b's requirement on a>
workspace() {
  local dir=$1
  mkdir -p "$dir/crates/a/src" "$dir/crates/b/src"
  printf '[workspace]\nmembers = ["crates/a", "crates/b"]\nresolver = "2"\n\n[workspace.package]\nedition = "2021"\n\n[profile.release]\nlto = true\n' > "$dir/Cargo.toml"
  printf '[package]\nname = "a"\nversion = "%s"\nedition.workspace = true\n' "$2" > "$dir/crates/a/Cargo.toml"
  printf '[package]\nname = "b"\nversion = "%s"\nedition.workspace = true\n\n[dependencies]\na = { path = "../a", version = "%s" }\n' "$3" "$4" > "$dir/crates/b/Cargo.toml"
  : > "$dir/crates/a/src/lib.rs"
  : > "$dir/crates/b/src/lib.rs"
}

# The release checkout: crates at 5.0.0, notes and changelog for 5.0.0 and
# 4.1.0 (so a v4.1.0 tag fails on the crate versions alone), origin/main at
# HEAD, annotated v5.0.0, v5.0.0-rc.1 and v4.1.0, and lightweight v5.0.0-rc.2.
repo=$work/repo
workspace "$repo" 5.0.0 5.0.0 5.0.0
mkdir -p "$repo/docs/releases"
printf '# Changelog\n\n## [5.0.0] - 2026-10-01\n\n## [4.1.0] - 2026-05-29\n' > "$repo/CHANGELOG.md"
printf '# RedlineDB v5.0.0\n\nNotes.\n' > "$repo/docs/releases/v5.0.0.md"
printf '# RedlineDB v4.1.0\n\nNotes.\n' > "$repo/docs/releases/v4.1.0.md"
git_ init --quiet "$repo"
git_ -C "$repo" commit --quiet --allow-empty -m parent
parent=$(git -C "$repo" rev-parse HEAD)
git_ -C "$repo" add -A
git_ -C "$repo" commit --quiet -m release
git -C "$repo" remote add origin https://github.com/neverhuman/redline.git
git -C "$repo" update-ref refs/remotes/origin/main HEAD
for tag in v5.0.0 v5.0.0-rc.1 v4.1.0 v5.0.0-rc.01; do git_ -C "$repo" tag -a -m "$tag" "$tag"; done
git_ -C "$repo" tag v5.0.0-rc.2

# run <label> <tag> [VAR=value...]: check in the fixture as GitHub Actions does
# (canonical repository id), unless the caller overrides the environment.
run() {
  local label=$1 tag=$2
  shift 2
  status=0
  : > "$work/gh.log"
  (cd "$repo" && env -u GITHUB_ACTIONS GITHUB_REPOSITORY_ID="$REDLINE_REPO_ID" "$@" \
    bash "$script" check "$tag") > "$work/$label.log" 2>&1 || status=$?
}
expect_pass() {
  local label=$1
  run "$@"
  [[ $status == 0 ]] || fail "$label: refused: $(cat "$work/$label.log")"
}
# expect_fail <label> <message> <tag> [VAR=value...]
expect_fail() {
  local label=$1 message=$2
  shift 2
  run "$label" "$@"
  [[ $status != 0 ]] || fail "$label: accepted $1"
  grep -qF -- "$message" "$work/$label.log" || fail "$label: expected '$message', got: $(cat "$work/$label.log")"
}

expect_pass stable v5.0.0
grep -qF "v5.0.0 releases 5.0.0 from $(git -C "$repo" rev-parse HEAD)" "$work/stable.log" ||
  fail "stable: unexpected report: $(cat "$work/stable.log")"
grep -qx "release view v5.0.0 --repo $REDLINE_REPO_SLUG --json tagName" "$work/gh.log" ||
  fail "stable: did not ask $REDLINE_REPO_SLUG for the release: $(cat "$work/gh.log")"
expect_pass candidate v5.0.0-rc.1
expect_fail older-version 'workspace crates are not at 4.1.0' v4.1.0
for bad in v5.0 v5.0.0-beta v5.0.0-rc.01 v5.0.0-rc.0 5.0.0 v5.0.0-rc v05.0.0 v5.0.0.1 'v5.0.0 '; do
  expect_fail "malformed-${bad// /_}" 'is not a release tag' "$bad"
done
expect_fail lightweight 'v5.0.0-rc.2 is not an annotated tag' v5.0.0-rc.2
expect_fail missing-tag 'v5.0.1 is not an annotated tag' v5.0.1
expect_fail released "already has a release for v5.0.0" v5.0.0 MOCK_RELEASES='v4.1.0 v5.0.0'
expect_fail released-candidate "already has a release for v5.0.0-rc.1" v5.0.0-rc.1 MOCK_RELEASES=v5.0.0-rc.1
expect_fail gh-unreachable 'cannot tell whether v5.0.0 already has a release: HTTP 502' v5.0.0 MOCK_GH_ERROR='HTTP 502'
expect_fail legacy-repository "repository id $legacy_id" v5.0.0 GITHUB_REPOSITORY_ID=$legacy_id
expect_fail actions-without-id 'repository id unknown' v5.0.0 GITHUB_ACTIONS=true GITHUB_REPOSITORY_ID=

# Outside GitHub Actions the id is GitHub's answer for origin.
expect_pass local-canonical v5.0.0 GITHUB_REPOSITORY_ID=
grep -qx 'api repos/neverhuman/redline --jq .id' "$work/gh.log" || fail "local-canonical: origin's id was not asked: $(cat "$work/gh.log")"
git -C "$repo" remote set-url origin "https://github.com/neverhuman/Redline""DB.git"
expect_fail local-legacy "repository id $legacy_id" v5.0.0 GITHUB_REPOSITORY_ID=
git -C "$repo" remote set-url origin https://github.com/neverhuman/redline.git

# Release notes and changelog.
printf '<!-- release-notes-placeholder -->\n' >> "$repo/docs/releases/v5.0.0.md"
expect_fail placeholder-notes 'docs/releases/v5.0.0.md is still the placeholder' v5.0.0
git -C "$repo" checkout --quiet -- docs/releases/v5.0.0.md
rm "$repo/docs/releases/v5.0.0.md"
expect_fail missing-notes 'docs/releases/v5.0.0.md, the release notes, does not exist' v5.0.0-rc.1
git -C "$repo" checkout --quiet -- docs/releases/v5.0.0.md
sed -i.orig 's/^## \[5\.0\.0\].*$/## Unreleased/' "$repo/CHANGELOG.md"
expect_fail missing-changelog "CHANGELOG.md has no '## [5.0.0]' section" v5.0.0
git -C "$repo" checkout --quiet -- CHANGELOG.md
rm -f "$repo/CHANGELOG.md.orig"
expect_pass restored v5.0.0

# A stable tag must be on origin/main; a candidate need not be.
git -C "$repo" update-ref refs/remotes/origin/main "$parent"
expect_fail off-main 'which is not on origin/main' v5.0.0
expect_pass candidate-off-main v5.0.0-rc.1
git -C "$repo" update-ref -d refs/remotes/origin/main
expect_fail main-unfetched 'origin/main is not fetched' v5.0.0
git -C "$repo" update-ref refs/remotes/origin/main HEAD

# The tag must name the checked-out commit.
git_ -C "$repo" commit --quiet --allow-empty -m later
expect_fail moved-head "v5.0.0 names $(git -C "$repo" rev-parse 'v5.0.0^{commit}'), not the checked-out commit" v5.0.0
git -C "$repo" reset --quiet --hard HEAD^

# Every failed rule is reported, not only the first.
run many v4.1.0 MOCK_RELEASES=v4.1.0 GITHUB_REPOSITORY_ID=$legacy_id
for message in 'workspace crates are not at 4.1.0' 'already has a release for v4.1.0' "repository id $legacy_id"; do
  grep -qF -- "$message" "$work/many.log" || fail "many: '$message' missing from: $(cat "$work/many.log")"
done

# --- workspace, dev-tag and bump --------------------------------------------
fresh=$work/fresh
workspace "$fresh" 4.1.0 4.1.0 4.0.0
git_ init --quiet "$fresh"
[[ $(cd "$fresh" && bash "$script" workspace) == 4.1.0 ]] || fail 'workspace: did not print 4.1.0'
[[ $(cd "$fresh" && bash "$script" dev-tag) == v4.1.0-dev ]] || fail 'dev-tag: did not print v4.1.0-dev'
(cd "$fresh" && bash "$script" bump 5.0.0) > "$work/bump.log" 2>&1 || fail "bump: $(cat "$work/bump.log")"
[[ $(cd "$fresh" && bash "$script" dev-tag) == v5.0.0-dev ]] || fail 'bump: dev-tag is not v5.0.0-dev afterwards'
grep -qx 'version = "5.0.0"' "$fresh/Cargo.toml" || fail 'bump: [workspace.package] has no version'
awk '/^\[workspace\.package\]$/ { t = 1; next } /^\[/ { t = 0 } t && /^version = "5\.0\.0"$/ { found = 1 } END { exit !found }' "$fresh/Cargo.toml" ||
  fail "bump: the version is outside [workspace.package]: $(cat "$fresh/Cargo.toml")"
for crate in a b; do
  grep -qx 'version.workspace = true' "$fresh/crates/$crate/Cargo.toml" || fail "bump: crate $crate does not inherit the version"
done
grep -qF 'a = { path = "../a", version = "5.0.0" }' "$fresh/crates/b/Cargo.toml" ||
  fail "bump: b's requirement on a did not follow: $(cat "$fresh/crates/b/Cargo.toml")"
grep -A1 -x 'name = "a"' "$fresh/Cargo.lock" | grep -qx 'version = "5.0.0"' || fail 'bump: Cargo.lock still records the old version'
(cd "$fresh" && bash "$script" bump 5.0) > "$work/bump-bad.log" 2>&1 && fail 'bump: accepted 5.0'
sed -i.orig 's/^version.workspace = true$/version = "5.0.1"/' "$fresh/crates/a/Cargo.toml"
(cd "$fresh" && bash "$script" dev-tag) > "$work/disagree.log" 2>&1 && fail 'dev-tag: accepted crates that disagree'
grep -qF 'workspace crates disagree on their version' "$work/disagree.log" || fail "disagree: $(cat "$work/disagree.log")"

# This checkout: every workspace crate shares one version.
(cd "$root" && bash "$script" dev-tag) > "$work/self.log" 2>&1 || fail "this checkout: $(cat "$work/self.log")"
grep -Eqx 'v[0-9]+\.[0-9]+\.[0-9]+-dev' "$work/self.log" || fail "this checkout: dev-tag printed $(cat "$work/self.log")"

[[ $failures == 0 ]] || { printf '%d release version check(s) failed\n' "$failures" >&2; exit 1; }
printf 'Release version tests passed.\n'
