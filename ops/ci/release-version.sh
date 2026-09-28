#!/usr/bin/env bash
# Release version policy (DX-05). A release tag names one new release of the
# version the checkout's workspace crates carry; nothing here is tied to a
# particular version.
#
#   release-version.sh check <tag>     refuse a tag that must not be published
#   release-version.sh bump <X.Y.Z>    give every workspace crate version X.Y.Z
#   release-version.sh workspace       print the workspace crates' one version
#   release-version.sh dev-tag         print v<workspace version>-dev, the TAG
#                                      untagged CI builds package under
#
# `check vX.Y.Z` or `check vX.Y.Z-rc.N` (N >= 1, no leading zeros) requires:
#   - every crate of the root workspace has version X.Y.Z (cargo metadata);
#   - CHANGELOG.md has a "## [X.Y.Z]" section;
#   - docs/releases/vX.Y.Z.md exists and is no longer the placeholder;
#   - the tag is annotated and peels to HEAD;
#   - the repository is neverhuman/redline by numeric id (GITHUB_REPOSITORY_ID
#     in GitHub Actions, otherwise the id GitHub reports for origin);
#   - the tag has no release yet (`gh release view` answers "release not
#     found"; any other failure refuses, because it proves nothing);
#   - a stable tag's commit is on origin/main.
# It reports every failed rule, then exits 1. It works on the git repository
# of the current directory, so run it from the checkout being released.
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=ops/release/authority.env
. "$here/../release/authority.env"
cargo=${CARGO:-cargo}

die() { printf 'release-version: %s\n' "$*" >&2; exit 1; }
top() { git rev-parse --show-toplevel 2>/dev/null || die 'not inside a git checkout'; }

# "name version" for every crate of the root workspace.
workspace_crates() {
  "$cargo" metadata --no-deps --format-version 1 --manifest-path "$(top)/Cargo.toml" |
    jq -r '.packages[] | "\(.name) \(.version)"'
}

workspace_version() {
  local crates versions
  crates=$(workspace_crates) || die 'cargo metadata failed'
  versions=$(cut -d ' ' -f 2 <<< "$crates" | sort -u)
  [[ -n $versions ]] || die 'the workspace has no crates'
  [[ $versions != *$'\n'* ]] || die "workspace crates disagree on their version:"$'\n'"$crates"
  printf '%s\n' "$versions"
}

# The GitHub slug of origin (owner/name), or nothing.
origin_slug() {
  local url
  url=$(git remote get-url origin 2>/dev/null) || return 0
  url=${url%.git}
  [[ $url =~ github\.com[:/]([^/]+/[^/]+)$ ]] && printf '%s\n' "${BASH_REMATCH[1]}"
  return 0
}

check() {
  local tag=$1 base rc='' problems=() crates wrong root commit head id slug view
  [[ $tag =~ ^v((0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*))(-rc\.[1-9][0-9]*)?$ ]] ||
    die "$tag is not a release tag: use vX.Y.Z or vX.Y.Z-rc.N (N from 1, no leading zeros)"
  base=${BASH_REMATCH[1]}
  [[ -z ${BASH_REMATCH[5]} ]] || rc=${BASH_REMATCH[5]}
  root=$(top)
  cd "$root"

  if crates=$(workspace_crates); then
    wrong=$(awk -v want="$base" '$2 != want' <<< "$crates")
    [[ -z $wrong ]] || problems+=("workspace crates are not at $base (run: bash ops/ci/release-version.sh bump $base):"$'\n'"$wrong")
  else
    problems+=('cargo metadata failed, so the crate versions are unknown')
  fi

  grep -Eq "^## \[${base//./\\.}\]" CHANGELOG.md 2>/dev/null ||
    problems+=("CHANGELOG.md has no '## [$base]' section")
  if [[ ! -f docs/releases/v$base.md ]]; then
    problems+=("docs/releases/v$base.md, the release notes, does not exist")
  elif grep -q 'release-notes-placeholder' "docs/releases/v$base.md"; then
    problems+=("docs/releases/v$base.md is still the placeholder; write the release notes")
  fi

  if [[ $(git cat-file -t "refs/tags/$tag" 2>/dev/null) != tag ]]; then
    problems+=("$tag is not an annotated tag in this checkout (create it with git tag -a)")
  fi
  commit=$(git rev-parse --verify --quiet "refs/tags/$tag^{commit}" || true)
  head=$(git rev-parse HEAD)
  [[ $commit == "$head" ]] || problems+=("$tag names ${commit:-nothing}, not the checked-out commit $head")

  if [[ -n ${GITHUB_REPOSITORY_ID:-} || ${GITHUB_ACTIONS:-} == true ]]; then
    id=${GITHUB_REPOSITORY_ID:-}
  else
    slug=$(origin_slug)
    id=''
    [[ -z $slug ]] || id=$(gh api "repos/$slug" --jq .id 2>/dev/null || true)
  fi
  [[ $id == "$REDLINE_REPO_ID" ]] ||
    problems+=("this is repository id ${id:-unknown}; releases come only from $REDLINE_REPO_SLUG (id $REDLINE_REPO_ID)")

  if view=$(gh release view "$tag" --repo "$REDLINE_REPO_SLUG" --json tagName 2>&1); then
    problems+=("$REDLINE_REPO_SLUG already has a release for $tag; a published tag is never reused")
  elif ! grep -q 'release not found' <<< "$view"; then
    problems+=("cannot tell whether $tag already has a release: ${view:-gh failed}")
  fi

  if [[ -z $rc && -n $commit ]]; then
    if ! git rev-parse --verify --quiet refs/remotes/origin/main >/dev/null; then
      problems+=("origin/main is not fetched, so stable ancestry cannot be checked")
    elif ! git merge-base --is-ancestor "$commit" refs/remotes/origin/main; then
      problems+=("stable $tag names $commit, which is not on origin/main")
    fi
  fi

  if ((${#problems[@]})); then
    printf 'release-version: %s cannot be released:\n' "$tag" >&2
    printf '  - %s\n' "${problems[@]}" >&2
    exit 1
  fi
  printf 'release-version: %s releases %s%s from %s\n' "$tag" "$base" "${rc:+ (candidate ${rc#-})}" "$head"
}

# Give every root-workspace crate version X.Y.Z: [workspace.package] holds
# the version, each crate inherits it, and version requirements on sibling
# crates (path = "../<crate>") follow. Then refresh Cargo.lock for the
# workspace crates only.
bump() {
  local version=$1 root manifest
  [[ $version =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || die "bump takes X.Y.Z, not $version"
  root=$(top)
  cd "$root"
  grep -q '^\[workspace\.package\]$' Cargo.toml || die 'Cargo.toml has no [workspace.package] table'
  local manifests
  manifests=$("$cargo" metadata --no-deps --format-version 1 --manifest-path Cargo.toml | jq -r '.packages[].manifest_path') ||
    die 'cargo metadata failed'
  awk -v v="$version" '
    /^\[/ { in_table = ($0 == "[workspace.package]") }
    in_table && /^version[ \t]*=/ { next }
    { print }
    $0 == "[workspace.package]" { print "version = \"" v "\"" }
  ' Cargo.toml > Cargo.toml.bump && mv Cargo.toml.bump Cargo.toml
  while IFS= read -r manifest; do
    [[ $manifest != "$root/Cargo.toml" ]] || continue
    awk -v v="$version" '
      /^\[/ { in_package = ($0 == "[package]") }
      in_package && /^version[ \t]*=/ { print "version.workspace = true"; next }
      /path[ \t]*=[ \t]*"\.\.\/[^\/"]+"/ && /version[ \t]*=[ \t]*"[^"]*"/ { sub(/version[ \t]*=[ \t]*"[^"]*"/, "version = \"" v "\"") }
      { print }
    ' "$manifest" > "$manifest.bump" && mv "$manifest.bump" "$manifest"
  done <<< "$manifests"
  "$cargo" update --workspace --manifest-path Cargo.toml
  [[ $(workspace_version) == "$version" ]] || die "after the bump the workspace is not at $version"
  printf 'release-version: workspace crates are at %s.\n' "$version"
  printf 'Next: a "## [%s]" CHANGELOG.md section and docs/releases/v%s.md.\n' "$version" "$version"
}

case "${1:-}" in
  check) [[ $# == 2 ]] || die 'usage: release-version.sh check <tag>'; check "$2" ;;
  bump) [[ $# == 2 ]] || die 'usage: release-version.sh bump <X.Y.Z>'; bump "$2" ;;
  workspace) cd "$(top)"; workspace_version ;;
  dev-tag) cd "$(top)"; version=$(workspace_version); printf 'v%s-dev\n' "$version" ;;
  *) die 'usage: release-version.sh check <tag> | bump <X.Y.Z> | workspace | dev-tag' ;;
esac
