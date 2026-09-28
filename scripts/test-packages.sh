#!/usr/bin/env bash
# Run downloaded archives without invoking development toolchains.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
packages=${1:-$root/target/packages}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/no-toolchain"
for tool in cargo rustc node npm cc clang gcc; do
  printf '#!/bin/sh\necho "development toolchain invoked unexpectedly" >&2\nexit 1\n' > "$work/no-toolchain/$tool"
  chmod +x "$work/no-toolchain/$tool"
done
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
# The published installer accepts this build's core archive: a local curl
# serves the canonical release download URLs from $packages.
mkdir -p "$work/release-transport"
cat > "$work/release-transport/curl" <<'BIN'
#!/usr/bin/env bash
set -eu
url= out=
while [[ $# -gt 0 ]]; do
  case "$1" in -o) out=$2; shift 2 ;; https://*) url=$1; shift ;; *) shift ;; esac
done
case "$url" in
  https://github.com/neverhuman/redline/releases/download/*) cp "$RELEASE_PACKAGES/${url##*/}" "$out" ;;
  *) printf 'release transport fixture: no route to %s\n' "$url" >&2; exit 22 ;;
esac
BIN
chmod +x "$work/release-transport/curl"
core=$(find "$packages" -maxdepth 1 -name 'redlinedb-v*.tar.gz' | head -n 1)
[[ -n $core ]] || { printf 'no redlinedb core archive in %s\n' "$packages" >&2; exit 1; }
PATH="$work/release-transport:$PATH" RELEASE_PACKAGES=$(cd "$packages" && pwd) \
  VERSION=$(tar -xzOf "$core" ./share/redlinedb/VERSION) PREFIX="$work/installer prefix" bash "$root/install.sh"
"$prefix/bin/redlinedb" --version
"$prefix/bin/redline-testing" --version
CHECK_FFI=0 bash "$root/scripts/test-binaries.sh" "$prefix/bin"
