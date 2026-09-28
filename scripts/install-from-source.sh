#!/usr/bin/env bash
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
cd "$root"
all=false
case "${1:-}" in
  --all) all=true; shift ;;
  --help|-h) printf 'Usage: %s [--all] (PREFIX defaults to ~/.local)\n' "$0"; exit 0 ;;
esac
[[ $# == 0 ]] || { printf 'Usage: %s [--all]\n' "$0" >&2; exit 64; }
prefix=${PREFIX:-$HOME/.local}
dest=${REDLINEDB_INSTALL_DIR:-$prefix/bin}
target_dir=${CARGO_TARGET_DIR:-$root/target}
bins=(redlinedb redlinedb-cli redlinedb-server)
if "$all"; then bins+=(redline-testing redline-web redline-proof redlinedb-client-smoke db-shim-parity); fi
for bin in "${bins[@]}"; do
  [[ -x $target_dir/release/$bin ]] || { printf 'missing %s; run ./scripts/build-from-source.sh first\n' "$target_dir/release/$bin" >&2; exit 1; }
done
# The shared library is installed under its ABI-major name (the soname /
# install name set by crates/ffi/build.rs). REDLINEDB_DEV_LINKS=0 skips the
# unversioned development symlink; package staging uses that so archives hold
# regular files only (the binary installer creates the link after extraction).
abi_major=$(sed -n 's/^#define RLDB_ABI_MAJOR \([0-9][0-9]*\)$/\1/p' contracts/c-abi/redlinedb.h)
[[ $abi_major =~ ^[0-9]+$ ]] || { printf 'contracts/c-abi/redlinedb.h lacks RLDB_ABI_MAJOR\n' >&2; exit 1; }
dev_links=${REDLINEDB_DEV_LINKS:-1}
mkdir -p "$dest" "$prefix/lib" "$prefix/include"
for bin in "${bins[@]}"; do install -m 755 "$target_dir/release/$bin" "$dest/$bin"; done
[[ ! -f $target_dir/release/libredlinedb.a ]] || install -m 644 "$target_dir/release/libredlinedb.a" "$prefix/lib/"
if [[ -f $target_dir/release/libredlinedb.so ]]; then
  install -m 644 "$target_dir/release/libredlinedb.so" "$prefix/lib/libredlinedb.so.$abi_major"
  [[ $dev_links != 1 ]] || ln -sfn "libredlinedb.so.$abi_major" "$prefix/lib/libredlinedb.so"
fi
if [[ -f $target_dir/release/libredlinedb.dylib ]]; then
  dylib=$prefix/lib/libredlinedb.$abi_major.dylib
  install -m 644 "$target_dir/release/libredlinedb.dylib" "$dylib"
  install_name_tool -id "@rpath/libredlinedb.$abi_major.dylib" "$dylib"
  codesign --force --sign - "$dylib"
  [[ $dev_links != 1 ]] || ln -sfn "libredlinedb.$abi_major.dylib" "$prefix/lib/libredlinedb.dylib"
fi
install -m 644 contracts/c-abi/redlinedb.h contracts/c-abi/sqlite3.h "$prefix/include/"
printf 'Installed binaries in %s, libraries and headers in %s\n' "$dest" "$prefix"
