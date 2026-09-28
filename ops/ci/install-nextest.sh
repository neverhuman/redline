#!/usr/bin/env bash
# Install the pinned cargo-nextest release into a job-local directory.
#
#   bash ops/ci/install-nextest.sh <dest-dir>
#
# CI calls this instead of using whatever cargo-nextest is already on PATH:
# on a shared self-hosted runner that binary may have been left by another
# job. The release archive is checked against a pinned SHA-256 before it is
# unpacked; the binary goes into <dest-dir> (under $RUNNER_TEMP, which the
# runner empties for every job), and <dest-dir> is put first on PATH for the
# job's later steps.
#
# CI_NEXTEST_CACHE_DIR (optional, trusted jobs only) keeps the verified
# archive between jobs so a flaky link to github.com does not fail a shard.
# A cached archive is copied out and checked against the same digest on
# every use, and deleted if it does not match. CI_NEXTEST_URL overrides the
# download URL for tests; the digest cannot be overridden.
#
# To move to another release, take the digest from the release's own
# `.sha256` asset and confirm it against the asset `digest` that
# `gh api repos/nextest-rs/nextest/releases/tags/cargo-nextest-<version>`
# reports.
set -euo pipefail

version=0.9.133
target=x86_64-unknown-linux-gnu
archive_sha256=a9f992321e8759818400d93abb9477b4b11422d18d216e8d208505bd73454103

asset=cargo-nextest-$version-$target.tar.gz
url=${CI_NEXTEST_URL:-https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-$version/$asset}
dest=${1:?usage: install-nextest.sh <dest-dir>}
cache=${CI_NEXTEST_CACHE_DIR:-}

if [[ $(uname -sm) != "Linux x86_64" ]]; then
  printf 'install-nextest.sh: only %s is pinned, this host is %s\n' "$target" "$(uname -sm)" >&2
  exit 1
fi

verify() {
  printf '%s  %s\n' "$archive_sha256" "$1" | sha256sum -c -
}

tmp=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/install-nextest.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
archive=$tmp/$asset

if [[ -n $cache && -f $cache/$asset ]]; then
  # Check the private copy, not the shared file, so it cannot change in between.
  cp "$cache/$asset" "$archive"
  if ! verify "$archive"; then
    printf '::warning::discarding cached %s: SHA-256 mismatch\n' "$asset" >&2
    rm -f "$cache/$asset" "$archive"
  fi
fi

if [[ ! -f $archive ]]; then
  curl --proto '=https,file' --proto-redir '=https' --tlsv1.2 -fsSL \
    --retry 5 --retry-all-errors --connect-timeout 20 -o "$archive" "$url"
  verify "$archive"
  if [[ -n $cache ]]; then
    mkdir -p "$cache"
    cp "$archive" "$cache/$asset.partial.$$"
    mv -f "$cache/$asset.partial.$$" "$cache/$asset"
  fi
fi

mkdir -p "$tmp/unpack" "$dest"
tar -xzf "$archive" -C "$tmp/unpack" cargo-nextest
install -m 0755 "$tmp/unpack/cargo-nextest" "$dest/cargo-nextest"

reported=$("$dest/cargo-nextest" nextest --version | head -n 1)
if [[ $reported != "cargo-nextest $version "* ]]; then
  printf 'install-nextest.sh: expected cargo-nextest %s, got: %s\n' "$version" "$reported" >&2
  rm -f "$dest/cargo-nextest"
  exit 1
fi

if [[ -n ${GITHUB_PATH:-} ]]; then
  printf '%s\n' "$dest" >>"$GITHUB_PATH"
fi
printf 'installed %s at %s (archive sha256 %s)\n' "$reported" "$dest/cargo-nextest" "$archive_sha256"
