#!/usr/bin/env bash
# Tests for scripts/release/check-package-layout.sh on fixture archives.
#
# Usage: bash scripts/release/test-package-layout.sh
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
check=$here/check-package-layout.sh
# shellcheck source=scripts/release/package-layout.sh
. "$here/package-layout.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }
tag=v5.0.0-rc.1

# archives <dir> <layout: new|old> [platform...]: one fixture archive per package.
archives() {
  local dir=$1 layout=$2 platform package share tree
  shift 2
  rm -rf "$dir" "$work/trees"
  mkdir -p "$dir"
  for platform in "${@:-linux-x86_64}"; do
    for package in "${release_packages[@]}"; do
      tree=$work/trees/$package-$platform
      if [[ $layout == new ]]; then share=$(package_share "$package"); else share=share/redlinedb; fi
      mkdir -p "$tree/bin" "$tree/$share/licenses"
      printf 'Apache-2.0\n' >"$tree/$share/LICENSE"
      printf 'notice of %s\n' "$package" >"$tree/$share/NOTICE"
      printf '%s\n' "$tag" >"$tree/$share/VERSION"
      printf 'name\tversion\n%s-dep\t1.0\n' "$package" >"$tree/$share/DEPENDENCIES.tsv"
      printf 'licence text of %s\n' "$package" >"$tree/$share/licenses/$package-dep-1.0"
      jq -cn --arg package "$package" --arg platform "$platform" --arg tag "$tag" \
        '{schema:"redline.release-build/v2",tag:$tag,platform:$platform,package:$package}' \
        >"$tree/$share/build-provenance.json"
      printf '#!/bin/sh\n' >"$tree/bin/$package"
      if [[ $package == redline-testing ]]; then
        mkdir -p "$tree/share/redlinedb/corpus"
        printf '[]\n' >"$tree/share/redlinedb/corpus/cases.json"
      fi
      tar -czf "$dir/$package-$tag-$platform.tar.gz" -C "$tree" .
    done
  done
}
# expect <label> <dir> <exit> [fragment]
expect() {
  local got=0 output
  output=$(bash "$check" "$2" 2>&1) || got=$?
  [[ $got == "$3" ]] || fail "$1: exit $got, want $3: $output"
  [[ -z ${4:-} || $output == *"$4"* ]] || fail "$1: output lacks '$4': $output"
}

[[ $(package_share redlinedb) == share/redlinedb ]] || fail 'core records moved'
[[ $(package_share redline-web) == share/redlinedb/components/redline-web ]] || fail 'web records path'
[[ $(archive_package "x/redline-testing-$tag-linux-arm64.tar.gz") == redline-testing ]] || fail 'archive_package testing'
[[ $(archive_package "redlinedb-v5.0.0-macos-arm64.tar.gz") == redlinedb ]] || fail 'archive_package core'
archive_package "redline-websocket-v1-linux-x86_64.tar.gz" >/dev/null 2>&1 && fail 'archive_package guessed a package'

archives "$work/new" new
expect 'the component layout' "$work/new" 0 'ok (1 platform(s))'
archive_provenance "$work/new/redline-web-$tag-linux-x86_64.tar.gz" | jq -e '.package == "redline-web"' >/dev/null ||
  fail 'archive_provenance does not read the component record'

archives "$work/four" new linux-x86_64 linux-arm64 macos-x86_64 macos-arm64
expect 'four platforms' "$work/four" 0 'ok (4 platform(s))'

# The old layout: every package wrote share/redlinedb, so the last one extracted won.
archives "$work/old" old
expect 'the shared layout' "$work/old" 1 'writes share/redlinedb/build-provenance.json, which belongs to the core package'
expect 'the shared layout, extracted together' "$work/old" 1 'share/redlinedb/build-provenance.json does not name package redlinedb'
expect 'the shared layout, records replaced' "$work/old" 1 "share/redlinedb/NOTICE is not redlinedb-$tag-linux-x86_64.tar.gz's"

# A component record that names another package.
archives "$work/wrong" new
tree=$work/trees/redline-web-linux-x86_64
jq -c '.package = "redlinedb"' "$tree/share/redlinedb/components/redline-web/build-provenance.json" >"$work/p"
mv "$work/p" "$tree/share/redlinedb/components/redline-web/build-provenance.json"
tar -czf "$work/wrong/redline-web-$tag-linux-x86_64.tar.gz" -C "$tree" .
expect 'a component naming the core package' "$work/wrong" 1 'does not name package redline-web'

# A missing record.
archives "$work/missing" new
rm "$work/trees/redline-testing-linux-x86_64/share/redlinedb/components/redline-testing/NOTICE"
tar -czf "$work/missing/redline-testing-$tag-linux-x86_64.tar.gz" -C "$work/trees/redline-testing-linux-x86_64" .
expect 'a missing record' "$work/missing" 1 'components/redline-testing/NOTICE is missing'

mkdir -p "$work/empty"
expect 'no archives' "$work/empty" 1 'no release archives'

if ((failures)); then
  printf 'test-package-layout: %d failure(s)\n' "$failures" >&2
  exit 1
fi
printf 'test-package-layout: ok\n'
