#!/usr/bin/env bash
# Offline tests for ops/ci/install-github-tools.sh (CI-04). The installer is
# copied into a scratch repository root so it installs into that root's
# target/ci/tools, never this checkout's.
#   - a download that does not match the pinned digest installs nothing;
#   - a tampered archive in the tool cache is deleted, not used;
#   - CI_TOOLS_CACHE_DIR= (empty) turns the cache off.
# Set CI_JANKURAI_TEST_ARCHIVE to the genuine release archive to also check,
# without network access, that a verified download fills
# $RUNNER_TOOL_CACHE/redlinedb-tools/<archive sha256>/ and that a later job
# whose checkout was cleaned installs from that cache alone.
#
# Usage: bash ops/ci/tests/install-github-tools.sh
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
installer=$root/ops/ci/install-github-tools.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }

pinned() { sed -n "s/^$1=//p" "$installer"; }
tag=$(pinned tag)
archive_sha=$(pinned archive_sha)
[[ -n $tag && ${#archive_sha} == 64 ]] || { printf 'FAIL: cannot read tag/archive_sha from %s\n' "$installer" >&2; exit 1; }
asset=jankurai-${tag#v}-x86_64-unknown-linux-gnu.tar.gz

# A convincing fake: the release's layout and version string.
fake_dir=$work/fake/${asset%.tar.gz}
mkdir -p "$fake_dir"
printf '#!/bin/sh\necho "jankurai 1.6.11"\n' > "$fake_dir/jankurai"
chmod 0755 "$fake_dir/jankurai"
tar -czf "$work/fake.tar.gz" -C "$work/fake" "${asset%.tar.gz}"

# run_installer <label> [VAR=value...]: a fresh scratch root; sets $status
# and $dest (the root's target/ci/tools).
run_installer() {
  local scratch=$work/$1
  shift
  mkdir -p "$scratch/ops/ci"
  cp "$installer" "$scratch/ops/ci/install-github-tools.sh"
  dest=$scratch/target/ci/tools
  : > "$scratch/github_path"
  status=0
  env -u RUNNER_TOOL_CACHE -u CI_TOOLS_CACHE_DIR GITHUB_PATH="$scratch/github_path" "$@" \
    bash "$scratch/ops/ci/install-github-tools.sh" > "$scratch/out" 2>&1 || status=$?
}
expect_refused() {
  [[ $status != 0 ]] || fail "$1: installer succeeded: $(tail -n 2 "$work/$1/out")"
  [[ ! -e $dest/jankurai ]] || fail "$1: installed a binary that failed verification"
  [[ ! -s $work/$1/github_path ]] || fail "$1: added $(cat "$work/$1/github_path") to GITHUB_PATH"
}

# 1. The download does not match the pinned archive digest.
run_installer tampered-download CI_JANKURAI_URL="file://$work/fake.tar.gz"
expect_refused tampered-download
grep -q FAILED "$work/tampered-download/out" \
  || fail "tampered-download: expected a sha256sum failure, got: $(cat "$work/tampered-download/out")"

# 2. A tampered cache entry is discarded, and the (fake) download refused.
cache=$work/tool-cache/redlinedb-tools/$archive_sha
mkdir -p "$cache"
cp "$work/fake.tar.gz" "$cache/$asset"
run_installer tampered-cache RUNNER_TOOL_CACHE="$work/tool-cache" CI_JANKURAI_URL="file://$work/fake.tar.gz"
expect_refused tampered-cache
[[ ! -e $cache/$asset ]] || fail "tampered-cache: the bad cache entry was kept"

# 3. Optional: the genuine archive, still without network access.
if [[ -n ${CI_JANKURAI_TEST_ARCHIVE:-} ]]; then
  cp "$CI_JANKURAI_TEST_ARCHIVE" "$work/genuine.tar.gz"
  run_installer genuine-download RUNNER_TOOL_CACHE="$work/fresh-cache" CI_JANKURAI_URL="file://$work/genuine.tar.gz"
  [[ $status == 0 ]] || fail "genuine-download: exit $status: $(tail -n 3 "$work/genuine-download/out")"
  [[ -x $dest/jankurai && $("$dest/jankurai" --version) == 'jankurai 1.6.11' ]] \
    || fail "genuine-download: no working jankurai at $dest"
  [[ $(cat "$work/genuine-download/github_path") == "$dest" ]] || fail "genuine-download: GITHUB_PATH not set to $dest"
  [[ -f $work/fresh-cache/redlinedb-tools/$archive_sha/$asset ]] \
    || fail "genuine-download: the verified archive was not cached under redlinedb-tools/$archive_sha"
  # A later job: the checkout clean removed target/ci/tools and github.com
  # is unreachable.
  run_installer cached-only RUNNER_TOOL_CACHE="$work/fresh-cache" CI_JANKURAI_URL="file://$work/unreachable.tar.gz"
  [[ $status == 0 ]] || fail "cached-only: did not install from the cache: $(tail -n 3 "$work/cached-only/out")"
  [[ -x $dest/jankurai ]] || fail "cached-only: nothing installed"
  # An empty CI_TOOLS_CACHE_DIR turns the cache off.
  run_installer cache-off RUNNER_TOOL_CACHE="$work/off-cache" CI_TOOLS_CACHE_DIR= CI_JANKURAI_URL="file://$work/genuine.tar.gz"
  [[ $status == 0 ]] || fail "cache-off: exit $status: $(tail -n 3 "$work/cache-off/out")"
  [[ ! -e $work/off-cache ]] || fail "cache-off: wrote to the tool cache although CI_TOOLS_CACHE_DIR is empty"
  echo "ok: genuine archive installed, cached, and reinstalled from the cache offline"
fi

[[ $failures == 0 ]] || { printf '%d install-github-tools check(s) failed\n' "$failures" >&2; exit 1; }
printf 'install-github-tools.sh tests passed.\n'
