#!/usr/bin/env bash
# Acceptance for preparing the v5.1.1 source commit. It intentionally fails
# while the workspace is still v5.1.0 or the required release docs are absent.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
want=5.1.1
fail() { printf 'release-prep: %s\n' "$*" >&2; exit 1; }
has() { grep -Fq -- "$2" "$1" || fail "$3"; }

root_version=$(awk '
  $0 == "[workspace.package]" { in_workspace = 1; next }
  /^\[/ { in_workspace = 0 }
  in_workspace && /^version[[:space:]]*=/ { gsub(/"/, "", $3); print $3; exit }
' Cargo.toml)
[[ $root_version == "$want" ]] || fail "root workspace version is ${root_version:-missing}, expected $want"

metadata=$(cargo metadata --no-deps --locked --format-version 1)
crates=$(jq -r '.packages[] | "\(.name) \(.version)"' <<< "$metadata")
[[ -n $crates ]] || fail 'no workspace packages'
while read -r name version; do
  [[ $version == "$want" ]] || fail "$name version is $version, expected $want"
done <<< "$crates"
locked=$(awk '
  function emit() { if (in_package && name != "" && version != "") print name " " version }
  /^\[\[package\]\]$/ { emit(); in_package = 1; name = version = ""; next }
  in_package && /^name = / { name = $3; gsub(/"/, "", name) }
  in_package && /^version = / { version = $3; gsub(/"/, "", version) }
  END { emit() }
' Cargo.lock)
while IFS= read -r crate; do
  grep -Fxq -- "$crate" <<< "$locked" || fail "Cargo.lock lacks $crate"
done <<< "$crates"

has README.md "docs/releases/v$want.md" 'README release notes link'
has README.md "/v$want/install.sh" 'README installer tag'
has README.md "version-$want-blue" 'README current-version badge'
has README.md "**v$want is an experimental release.**" 'README current-release warning'
has README.md "## What's new in v$want" 'README current-release section'
has README.md 'startup-inclusive CLI latency' 'README CLI benchmark scope'
has docs/install.md "/v$want/install.sh" 'install guide tag'
has docs/install.md "git checkout v$want" 'install guide source tag'
has CHANGELOG.md "## [$want]" 'versioned changelog section'

notes="docs/releases/v$want.md"
[[ -f $notes ]] || fail 'release notes absent'
[[ $(head -n 1 "$notes") == "# RedlineDB v$want" ]] || fail 'release notes heading'
! grep -Fq 'release-notes-placeholder' "$notes" || fail 'placeholder release notes'
for phrase in rowid 'per table' silently process 'Known limitations'; do
  grep -Fqi -- "$phrase" "$notes" || fail "release notes missing $phrase"
done
for path in README.md "$notes"; do
  awk '
    /<!-- engine-throughput:begin -->/ { begin = NR; begins++ }
    /<!-- engine-throughput:end -->/ { end = NR; ends++ }
    END { exit !(begins == 1 && ends == 1 && begin < end) }
  ' "$path" || fail "$path: missing or reversed performance markers"
done
printf 'release preparation for v%s passes across %s workspace crates and required documents\n' \
  "$want" "$(wc -l <<< "$crates")"
