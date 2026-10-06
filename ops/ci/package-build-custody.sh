#!/usr/bin/env bash
# Bind the container checkout and downloaded archives to the caller's Git head.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
mode=${1:-}
case $mode in
  checkout) sha=${2:-}; tree=${3:-} ;;
  archives) packages=${2:-}; sha=${3:-}; tree=${4:-} ;;
  *) printf 'usage: package-build-custody.sh checkout SHA TREE | archives DIR SHA TREE\n' >&2; exit 64 ;;
esac
[[ $sha =~ ^[0-9a-f]{40}$ && $tree =~ ^[0-9a-f]{40}$ ]] || {
  printf 'package custody requires full source SHA and tree\n' >&2; exit 1
}
if [[ $mode == checkout ]]; then
  [[ $(git -C "$root" rev-parse HEAD) == "$sha" && $(git -C "$root" rev-parse 'HEAD^{tree}') == "$tree" ]] || {
    printf 'package checkout differs from requested SHA %s / tree %s\n' "$sha" "$tree" >&2; exit 1
  }
  if ! status=$(git -C "$root" status --porcelain --untracked-files=all); then
    printf 'cannot read package checkout status\n' >&2; exit 1
  fi
  [[ -z $status ]] || {
    printf 'package checkout has modified or untracked source inputs:\n%s\n' "$status" >&2; exit 1
  }
else
  # Read records without extracting executable files into the source tree.
  # shellcheck source=scripts/release/package-layout.sh
  . "$root/scripts/release/package-layout.sh"
  archives=("$packages"/*.tar.gz)
  [[ -f ${archives[0]} ]] || { printf 'no package archives in %s\n' "$packages" >&2; exit 1; }
  for archive in "${archives[@]}"; do
    share=$(package_share "$(archive_package "${archive##*/}")")
    provenance=$(tar -xzOf "$archive" "./$share/build-provenance.json")
    jq -e --arg sha "$sha" --arg tree "$tree" \
      '.commit == $sha and .source_tree == $tree' <<< "$provenance" >/dev/null || {
      printf '%s: archive provenance differs from requested SHA %s / tree %s\n' "$archive" "$sha" "$tree" >&2; exit 1
    }
  done
fi
printf 'Package %s custody matches %s / %s\n' "$mode" "$sha" "$tree"
