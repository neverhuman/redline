#!/usr/bin/env bash
# Install a published release the way docs/install.md does, then use it:
# the installer from the tag's own raw URL with VERSION pinned, into a PREFIX
# with a space; `SELECT 1`; a database that survives close and reopen; and
# `redlinedb --build-info --json` naming the tag, the released commit, the
# canonical repository and the tag's version. With VERIFY_LATEST=1 (or true)
# it also installs the latest release into a second prefix and requires that
# to be the same tag.
#
#   scripts/release/verify-published.sh <tag> <commit> <prefix>
#
# release-build.yml's verify-published job runs it on each platform after
# publication. scripts/test-verify-published.sh runs it against candidate
# archives through the file-transport curl of scripts/release/test-shims.sh.
set -euo pipefail
[[ $# == 3 ]] || { printf 'usage: %s <tag> <commit> <prefix>\n' "$0" >&2; exit 64; }
tag=$1 commit=$2 prefix=$3
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
# shellcheck source=ops/release/authority.env
. "$root/ops/release/authority.env"
die() { printf 'verify-published %s: %s\n' "$tag" "$*" >&2; exit 1; }
[[ $commit =~ ^[0-9a-f]{40}$ ]] || die "commit must be a full SHA-1, got '$commit'"
[[ ! -e $prefix ]] || die "$prefix already exists; use a fresh prefix"
installer="https://raw.githubusercontent.com/${REDLINE_REPO_SLUG}/$tag/install.sh"
version=${tag#v}
version=${version%%-*}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# install <prefix> <VERSION>: the documented pipe, from a directory of its own.
install_release() {
  (cd "$work" && curl -fsSL "$installer" | VERSION=$2 PREFIX=$1 bash) || die "installing into $1 failed"
}

# check_build_info <prefix>: the installed CLI is the released build.
check_build_info() {
  local info field
  info=$("$1/bin/redlinedb" --build-info --json) || die "$1/bin/redlinedb --build-info --json failed"
  for field in "\"tag\":\"$tag\"" "\"source_sha\":\"$commit\"" "\"repository_id\":$REDLINE_REPO_ID" "\"version\":\"$version\""; do
    grep -Fq "$field" <<< "$info" || die "--build-info lacks $field: $info"
  done
}

install_release "$prefix" "$tag"
bin=$prefix/bin/redlinedb
answer=$(cd "$work" && "$bin" -batch :memory: 'SELECT 1;') || die 'SELECT 1 failed'
[[ $answer == 1 ]] || die "SELECT 1 printed '$answer'"
database="$work/data dir/verify.redline"
mkdir -p "${database%/*}"
(cd "$work" && "$bin" -batch -bail "$database" 'CREATE TABLE t(n INTEGER); INSERT INTO t VALUES (42);') ||
  die 'creating a database on disk failed'
answer=$(cd "$work" && "$bin" -batch -bail "$database" 'SELECT n FROM t;') || die 'reopening the database failed'
[[ $answer == 42 ]] || die "the reopened database returned '$answer'"
check_build_info "$prefix"

case "${VERIFY_LATEST:-0}" in
  1|true)
    latest="$work/latest prefix"
    install_release "$latest" ''
    check_build_info "$latest"
    ;;
  0|false) ;;
  *) die "VERIFY_LATEST must be 0, 1, true or false" ;;
esac
printf 'verify-published: %s from %s installs and runs on %s.\n' "$tag" "$commit" "$(uname -sm)"
