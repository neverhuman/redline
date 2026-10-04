#!/usr/bin/env bash
# Acceptance for preparing the v5.1.1 source commit. It intentionally fails
# while the workspace is still v5.1.0 or the required release docs are absent.
# An optional README path lets the same checks reject an older README fixture.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
want=5.1.1
readme=${1:-README.md}
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

has "$readme" "docs/releases/v$want.md" 'README release notes link'
has "$readme" "/v$want/install.sh" 'README installer tag'
has "$readme" "version-$want-blue" 'README current-version badge'
has "$readme" "**v$want is an experimental release.**" 'README current-release warning'
has "$readme" "## What's new in v$want" 'README current-release section'
has "$readme" "## v$want engine benchmarks" 'README current benchmark heading'
has "$readme" '<a id="engine-throughput"></a>' 'README engine benchmark anchor'
has "$readme" '<a id="versions-over-time"></a>' 'README historical CLI anchor'
has "$readme" '## Historical CLI benchmarks (through v5.0.0)' 'README historical CLI heading'
has "$readme" 'startup-inclusive CLI latency' 'README CLI benchmark scope'
awk '
  index($0, "**v5.1.1 benchmarks:** [Measured engine throughput vs v5.1.0](#engine-throughput)") == 1 { link = NR }
  $0 == "## Quick start" { quickstart = NR }
  END { exit !(link > 0 && quickstart > link) }
' "$readme" || fail 'README needs the current benchmark link before Quick start'
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
for path in "$readme" "$notes"; do
  awk '
    /<!-- engine-throughput:begin -->/ { begin = NR; begins++ }
    /<!-- engine-throughput:end -->/ { end = NR; ends++ }
    END { exit !(begins == 1 && ends == 1 && begin < end) }
  ' "$path" || fail "$path: missing or reversed performance markers"
done
printf 'release preparation for v%s passes across %s workspace crates and required documents\n' \
  "$want" "$(wc -l <<< "$crates")"
