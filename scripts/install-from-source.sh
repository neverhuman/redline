#!/usr/bin/env bash
# Install a build made by ./scripts/build-from-source.sh.
#
# The build is laid out as a package tree and activated by install.sh
# (REDLINEDB_LOCAL_PACKAGE), so a source install gets the binary installer's
# versioned layout, validation, lock, one-rename switch and rollback:
# PREFIX/lib/redlinedb/versions/source-<commit>-<time>-<pid>, with PREFIX/bin,
# PREFIX/lib and PREFIX/include linked through PREFIX/lib/redlinedb/current.
# REDLINEDB_MIGRATE_LEGACY=1 moves files of an earlier flat install aside.
#
# --tree DIR only writes the package tree (regular files, no links) into DIR
# and activates nothing; scripts/package-release.sh stages archives with it.
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
cd "$root"
usage() { printf 'Usage: %s [--all] [--tree DIR] (PREFIX defaults to ~/.local)\n' "$0"; }
all=false tree=''
while [[ $# -gt 0 ]]; do
  case $1 in
    --all) all=true; shift ;;
    --tree) [[ $# -ge 2 && -n $2 ]] || { usage >&2; exit 64; }; tree=$2; shift 2 ;;
    --help | -h) usage; exit 0 ;;
    *) usage >&2; exit 64 ;;
  esac
done
target_dir=${CARGO_TARGET_DIR:-$root/target}
bins=(redlinedb redlinedb-cli redlinedb-server)
if "$all"; then bins+=(redline-testing redline-web redline-proof redlinedb-client-smoke db-shim-parity); fi
for bin in "${bins[@]}"; do
  [[ -x $target_dir/release/$bin ]] || { printf 'missing %s; run ./scripts/build-from-source.sh first\n' "$target_dir/release/$bin" >&2; exit 1; }
done
# The shared library is laid out under its ABI-major name (the soname /
# install name set by crates/ffi/build.rs). The unversioned development link
# is the installer's job, so trees and archives hold regular files only.
abi_major=$(sed -n 's/^#define RLDB_ABI_MAJOR \([0-9][0-9]*\)$/\1/p' contracts/c-abi/redlinedb.h)
[[ $abi_major =~ ^[0-9]+$ ]] || { printf 'contracts/c-abi/redlinedb.h lacks RLDB_ABI_MAJOR\n' >&2; exit 1; }

# lay_out <dir>: write the package tree into <dir>.
lay_out() {
  local out=$1 bin dylib
  mkdir -p "$out/bin" "$out/lib" "$out/include"
  for bin in "${bins[@]}"; do install -m 755 "$target_dir/release/$bin" "$out/bin/$bin"; done
  [[ ! -f $target_dir/release/libredlinedb.a ]] || install -m 644 "$target_dir/release/libredlinedb.a" "$out/lib/"
  [[ ! -f $target_dir/release/libredlinedb.so ]] ||
    install -m 644 "$target_dir/release/libredlinedb.so" "$out/lib/libredlinedb.so.$abi_major"
  if [[ -f $target_dir/release/libredlinedb.dylib ]]; then
    dylib=$out/lib/libredlinedb.$abi_major.dylib
    install -m 644 "$target_dir/release/libredlinedb.dylib" "$dylib"
    install_name_tool -id "@rpath/libredlinedb.$abi_major.dylib" "$dylib"
    codesign --force --sign - "$dylib"
  fi
  install -m 644 contracts/c-abi/redlinedb.h contracts/c-abi/sqlite3.h "$out/include/"
}

if [[ -n $tree ]]; then
  lay_out "$tree"
  printf 'Package tree written to %s\n' "$tree"
  exit 0
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
lay_out "$work/package"
commit=$(git rev-parse --short=12 HEAD 2>/dev/null || echo unknown)
[[ $commit == unknown || -z $(git status --porcelain 2>/dev/null) ]] || commit=$commit-dirty
label=source-$commit-$(date -u +%Y%m%dT%H%M%SZ)-$$
REDLINEDB_LOCAL_PACKAGE="$work/package" REDLINEDB_LOCAL_LABEL=$label REDLINEDB_ROLLBACK=0 \
  PREFIX="${PREFIX:-$HOME/.local}" bash "$root/install.sh"
