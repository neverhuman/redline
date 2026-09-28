#!/usr/bin/env bash
# Run downloaded archives without invoking development toolchains: check each
# archive's checksum and provenance, extract them together, and run the
# binaries. scripts/test-native-install.sh installs the core archive with
# install.sh instead.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
packages=${1:-$root/target/packages}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# shellcheck source=scripts/release/test-shims.sh
. "$root/scripts/release/test-shims.sh"
write_no_toolchain "$work/no-toolchain"
export PATH="$work/no-toolchain:$PATH"
prefix="$work/install with spaces"
mkdir -p "$prefix"
# shellcheck source=ops/release/authority.env
. "$root/ops/release/authority.env"
for archive in "$packages"/*.tar.gz; do
  directory=$(cd "$(dirname "$archive")" && pwd)
  name=${archive##*/}
  (cd "$directory"; if command -v sha256sum >/dev/null; then sha256sum -c "$name.sha256"; else shasum -a 256 -c "$name.sha256"; fi)
  # install.sh and publish-github-release.sh match these exact fields.
  provenance=$(tar -xzOf "$archive" ./share/redlinedb/build-provenance.json)
  tag=$(tar -xzOf "$archive" ./share/redlinedb/VERSION)
  for field in "\"repository_id\":${REDLINE_REPO_ID}[,}]" '"commit":"[0-9a-f]{40}"' '"source_tree":"[0-9a-f]{40}"'; do
    grep -Eq "$field" <<< "$provenance" || { printf '%s: provenance lacks %s: %s\n' "$name" "$field" "$provenance" >&2; exit 1; }
  done
  grep -Fq "\"tag\":\"$tag\"" <<< "$provenance" || { printf '%s: provenance does not name %s: %s\n' "$name" "$tag" "$provenance" >&2; exit 1; }
  tar -xzf "$archive" -C "$prefix"
done
"$prefix/bin/redlinedb" --version
"$prefix/bin/redline-testing" --version
CHECK_FFI=0 bash "$root/scripts/test-binaries.sh" "$prefix/bin"
