#!/usr/bin/env bash
# Install the pinned jankurai auditor at target/ci/tools/jankurai, the one
# path ops/ci/lib.sh accepts, checked by archive and binary SHA-256.
#
# The CI checkout clean empties target/ci/tools in every job, and the
# self-hosted runners' TLS link to github.com is flaky (CI-04: run
# 36298015185 lost the security job to an SSL timeout here). So the verified
# release archive is kept outside the workspace, in
# $RUNNER_TOOL_CACHE/redlinedb-tools/<archive sha256>/, and a download retries.
# A cached archive is copied out and checked against the pinned digest on
# every use, and deleted if it does not match.
#
# CI_TOOLS_CACHE_DIR replaces $RUNNER_TOOL_CACHE/redlinedb-tools (empty: no
# cache). CI_JANKURAI_URL replaces the download URL for tests; the digests
# cannot be replaced. ops/ci/tests/install-github-tools.sh tests this.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
dest="$root/target/ci/tools"
tag=v1.6.11-deadlang-precision-split.3
asset=jankurai-${tag#v}-x86_64-unknown-linux-gnu.tar.gz
archive_sha=a192cb302ba6e4fc58657c6f26c5d7a9a49d76302ba8c09dab463fe3fe95a66e
binary_sha=9e6b8857a26f6004d4c74e510e13b06d880f2e2ae0c89502698889ed690c5d6c
url=${CI_JANKURAI_URL:-https://github.com/neverhuman/jankurai/releases/download/$tag/$asset}
cache_root=${CI_TOOLS_CACHE_DIR-${RUNNER_TOOL_CACHE:+$RUNNER_TOOL_CACHE/redlinedb-tools}}
cache=${cache_root:+$cache_root/$archive_sha}

verify_archive() {
  printf '%s  %s\n' "$archive_sha" "$1" | sha256sum -c -
}

if [[ ! -f $dest/jankurai ]] || [[ $(sha256sum "$dest/jankurai" | cut -d' ' -f1) != "$binary_sha" ]]; then
  mkdir -p "$dest"
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  archive=$tmp/$asset
  if [[ -n $cache && -f $cache/$asset ]]; then
    # Check the private copy, not the shared file, so it cannot change in between.
    cp "$cache/$asset" "$archive"
    if ! verify_archive "$archive"; then
      printf '::warning::discarding cached %s: SHA-256 mismatch\n' "$asset" >&2
      rm -f "$cache/$asset" "$archive"
    fi
  fi
  if [[ ! -f $archive ]]; then
    curl --proto '=https,file' --proto-redir '=https' --tlsv1.2 -fsSL \
      --retry 5 --retry-all-errors --connect-timeout 20 -o "$archive" "$url"
    verify_archive "$archive"
    if [[ -n $cache ]]; then
      mkdir -p "$cache"
      cp "$archive" "$cache/$asset.partial.$$"
      mv -f "$cache/$asset.partial.$$" "$cache/$asset"
    fi
  fi
  tar -xzf "$archive" -C "$tmp"
  install -m 755 "$tmp/${asset%.tar.gz}/jankurai" "$dest/jankurai"
fi
printf '%s  %s\n' "$binary_sha" "$dest/jankurai" | sha256sum -c -
[[ $("$dest/jankurai" --version) == 'jankurai 1.6.11' ]]
[[ -z ${GITHUB_PATH:-} ]] || printf '%s\n' "$dest" >> "$GITHUB_PATH"
