#!/usr/bin/env bash
# Run scripts/release/verify-published.sh, the post-publication check of
# release-build.yml, against this platform's candidate archives before
# anything is published: a file-transport curl serves the canonical installer
# and release URLs from the packages directory, and no development toolchain
# is on PATH. Also checks that the script refuses a build that names another
# commit.
#
#   scripts/test-verify-published.sh [packages dir]
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
packages=$(cd "${1:-${OUTPUT_DIR:-$root/target/packages}}" && pwd)
# shellcheck source=scripts/release/test-shims.sh
. "$root/scripts/release/test-shims.sh"
die() { printf 'test-verify-published: %s\n' "$*" >&2; exit 1; }
archives=("$packages"/redlinedb-v*.tar.gz)
[[ ${#archives[@]} == 1 && -f ${archives[0]} ]] || die "expected one core archive in $packages"
tag=$(tar -xzOf "${archives[0]}" ./share/redlinedb/VERSION)
commit=$(tar -xzOf "${archives[0]}" ./share/redlinedb/build-provenance.json | sed -n 's/.*"commit":"\([0-9a-f]\{40\}\)".*/\1/p')
[[ -n $commit ]] || die "the core archive's provenance names no commit"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
write_no_toolchain "$work/no-toolchain"
write_release_transport "$work/transport"
export RELEASE_PACKAGES=$packages RELEASE_TAG=$tag RELEASE_INSTALLER=$root/install.sh
export PATH="$work/no-toolchain:$work/transport:$PATH"

VERIFY_LATEST=1 bash "$root/scripts/release/verify-published.sh" "$tag" "$commit" "$work/rl x" ||
  die "the candidate $tag failed the post-publication check"

other=$(printf '%040d' 0)
if VERIFY_LATEST=0 bash "$root/scripts/release/verify-published.sh" "$tag" "$other" "$work/other prefix" 2> "$work/other.log"; then
  die 'a build of another commit passed'
fi
grep -qF -- "--build-info lacks \"source_sha\":\"$other\"" "$work/other.log" || die "unexpected refusal: $(cat "$work/other.log")"
printf 'verify-published passes for %s at %s and refuses another commit.\n' "$tag" "$commit"
