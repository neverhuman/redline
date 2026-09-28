#!/usr/bin/env bash
# Offline tests for ops/ci/install-nextest.sh (S8-06). A tampered download,
# a tampered cache entry and a cargo-nextest planted on PATH must all fail
# closed: nothing installed, nothing added to GITHUB_PATH.
#
# Set CI_NEXTEST_TEST_ARCHIVE to the genuine release archive to also check
# the success path without network access.
#
# Usage: bash ops/ci/tests/install-nextest.sh
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
installer=$root/ops/ci/install-nextest.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

version=$(sed -n 's/^version=//p' "$installer")
target=$(sed -n 's/^target=//p' "$installer")
[[ -n $version && -n $target ]] || fail "cannot read version/target from $installer"
asset=cargo-nextest-$version-$target.tar.gz

# A convincing fake: right name, right layout, right version string.
mkdir -p "$work/fake" "$work/planted"
printf '#!/bin/sh\necho "cargo-nextest %s (planted)"\n' "$version" >"$work/fake/cargo-nextest"
chmod 0755 "$work/fake/cargo-nextest"
tar -czf "$work/fake.tar.gz" -C "$work/fake" cargo-nextest
cp "$work/fake/cargo-nextest" "$work/planted/cargo-nextest"

# run_installer <label> <dest> [VAR=value...]; sets $status.
run_installer() {
  local label=$1 dest=$2
  shift 2
  : >"$work/github_path"
  status=0
  env PATH="$work/planted:$PATH" GITHUB_PATH="$work/github_path" "$@" \
    bash "$installer" "$dest" >"$work/$label.out" 2>&1 || status=$?
}

expect_refused() {
  local label=$1 dest=$2
  [[ $status -ne 0 ]] || fail "$label: installer succeeded; output: $(cat "$work/$label.out")"
  [[ ! -e $dest/cargo-nextest ]] || fail "$label: installed a binary that failed verification"
  [[ ! -s $work/github_path ]] || fail "$label: added $(cat "$work/github_path") to GITHUB_PATH"
}

# 0. Any other platform is refused before anything is downloaded: here a
#    uname stand-in reports macOS on arm64.
mkdir -p "$work/other-host"
printf '#!/bin/sh\necho "Darwin arm64"\n' >"$work/other-host/uname"
chmod 0755 "$work/other-host/uname"
run_installer other-platform "$work/dest0" PATH="$work/other-host:$work/planted:$PATH" \
  CI_NEXTEST_URL="file://$work/fake.tar.gz"
expect_refused other-platform "$work/dest0"
grep -q "only $target is pinned" "$work/other-platform.out" \
  || fail "other-platform: expected the platform refusal, got: $(cat "$work/other-platform.out")"
# The cases below run the pinned platform's verification, so they need it.
if [[ $(uname -sm) != "Linux x86_64" ]]; then
  printf 'install-nextest tests: platform refusal checked; the rest needs Linux x86_64 (this host is %s)\n' "$(uname -sm)"
  exit 0
fi

# 1. The downloaded archive does not match the pinned digest, and a
#    cargo-nextest of the right version is already on PATH. The installer
#    must not trust the planted binary and must not install the fake one.
run_installer tampered-download "$work/dest1" CI_NEXTEST_URL="file://$work/fake.tar.gz"
expect_refused tampered-download "$work/dest1"
grep -q 'FAILED' "$work/tampered-download.out" \
  || fail "tampered-download: expected a sha256sum failure, got: $(cat "$work/tampered-download.out")"

# 2. A tampered archive in the cache is discarded, not used, and the fresh
#    (also fake) download is refused too.
mkdir -p "$work/cache"
cp "$work/fake.tar.gz" "$work/cache/$asset"
run_installer tampered-cache "$work/dest2" \
  CI_NEXTEST_CACHE_DIR="$work/cache" CI_NEXTEST_URL="file://$work/fake.tar.gz"
expect_refused tampered-cache "$work/dest2"
[[ ! -e $work/cache/$asset ]] || fail "tampered-cache: the bad cache entry was kept"

# 3. A missing destination argument is a usage error.
status=0
bash "$installer" >"$work/usage.out" 2>&1 || status=$?
[[ $status -ne 0 ]] || fail "usage: installer ran without a destination"

# 4. Optional success path from a verified cache entry, with no network.
if [[ -n ${CI_NEXTEST_TEST_ARCHIVE:-} ]]; then
  mkdir -p "$work/good-cache"
  cp "$CI_NEXTEST_TEST_ARCHIVE" "$work/good-cache/$asset"
  run_installer genuine "$work/dest4" \
    CI_NEXTEST_CACHE_DIR="$work/good-cache" CI_NEXTEST_URL="file://$work/missing.tar.gz"
  [[ $status -eq 0 ]] || fail "genuine: installer failed: $(cat "$work/genuine.out")"
  [[ $("$work/dest4/cargo-nextest" nextest --version | head -n 1) == "cargo-nextest $version "* ]] \
    || fail "genuine: installed binary reports the wrong version"
  [[ $(cat "$work/github_path") == "$work/dest4" ]] || fail "genuine: GITHUB_PATH not set to the job-local dir"
  echo "ok: genuine archive installed from the verified cache"
fi

echo "ok: install-nextest.sh fails closed on tampered archives and ignores PATH"
