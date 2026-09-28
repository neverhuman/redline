#!/usr/bin/env bash
# Check that the release archives in <dir> keep their records apart (DX-08,
# scripts/release/package-layout.sh). For each platform, every package's
# records are where package_share puts them and name that package; then the
# platform's archives are extracted together in every order, and afterwards
# share/redlinedb/build-provenance.json still names package "redlinedb" and
# every package's records are byte-identical to its own archive's.
#
# Usage: bash scripts/release/check-package-layout.sh <dir>
# scripts/test-packages.sh runs it; scripts/release/test-package-layout.sh tests it.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=scripts/release/package-layout.sh
. "$here/package-layout.sh"
packages=${1:?usage: check-package-layout.sh <dir>}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
fail() { printf 'package-layout: %s\n' "$*" >&2; failures=$((failures + 1)); }
records=(LICENSE NOTICE VERSION build-provenance.json)

declare -A by_platform=()
for archive in "$packages"/*.tar.gz; do
  [[ -f $archive ]] || continue
  name=${archive##*/}
  package=$(archive_package "$name") || { fail "$name: not a release archive name"; continue; }
  platform=''
  for candidate in linux-x86_64 linux-arm64 macos-x86_64 macos-arm64; do
    [[ $name != *-$candidate.tar.gz ]] || platform=$candidate
  done
  [[ -n $platform ]] || { fail "$name: unknown platform"; continue; }
  share=$(package_share "$package")
  solo=$work/solo/$name
  mkdir -p "$solo"
  tar -xzf "$archive" -C "$solo"
  for record in "${records[@]}"; do
    [[ -f $solo/$share/$record ]] || fail "$name: $share/$record is missing"
  done
  jq -e --arg package "$package" '.package == $package' "$solo/$share/build-provenance.json" >/dev/null 2>&1 ||
    fail "$name: $share/build-provenance.json does not name package $package"
  if [[ $package != redlinedb ]]; then
    for record in "${records[@]}" DEPENDENCIES.tsv sbom.cdx.json licenses; do
      [[ ! -e $solo/share/redlinedb/$record ]] ||
        fail "$name: writes share/redlinedb/$record, which belongs to the core package"
    done
  fi
  by_platform[$platform]+="$archive"$'\n'
done
((${#by_platform[@]})) || { fail "no release archives in $packages"; exit 1; }

# permutations <items...>: every order, one per line, space-separated.
permutations() {
  if (($# <= 1)); then
    printf '%s\n' "$*"
    return
  fi
  local i rest
  for ((i = 1; i <= $#; i++)); do
    rest=("${@:1:i-1}" "${@:i+1}")
    permutations "${rest[@]}" | sed "s|^|${!i} |"
  done
}

for platform in "${!by_platform[@]}"; do
  mapfile -t group < <(printf '%s' "${by_platform[$platform]}")
  names=()
  for archive in "${group[@]}"; do names+=("${archive##*/}"); done
  while read -r -a order; do
    combined=$work/combined
    rm -rf "$combined"
    mkdir -p "$combined"
    for name in "${order[@]}"; do
      tar -xzf "$packages/$name" -C "$combined"
    done
    label="$platform, extracted as ${order[*]}"
    for name in "${order[@]}"; do
      package=$(archive_package "$name")
      share=$(package_share "$package")
      [[ -d $work/solo/$name/$share ]] || continue # reported above
      while IFS= read -r file; do
        cmp -s "$work/solo/$name/$file" "$combined/$file" || fail "$label: $file is not $name's"
      done < <(cd "$work/solo/$name" && find "$share" -path "$share/components" -prune -o -type f -print)
    done
    if [[ -f $combined/share/redlinedb/build-provenance.json ]] &&
      ! jq -e '.package == "redlinedb"' "$combined/share/redlinedb/build-provenance.json" >/dev/null 2>&1; then
      fail "$label: share/redlinedb/build-provenance.json does not name package redlinedb"
    fi
  done < <(permutations "${names[@]}")
done

if ((failures)); then
  printf 'package-layout: %d failure(s)\n' "$failures" >&2
  exit 1
fi
printf 'package-layout: ok (%d platform(s))\n' "${#by_platform[@]}"
