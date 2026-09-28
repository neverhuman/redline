#!/usr/bin/env bash
# Check the packaged C library: regular files only, the ABI-major load identity
# (soname / install name), C consumers linked against the extracted headers and
# libraries (dynamic and static) that still run after the tree is relocated,
# and the independent upstream-header ABI probe against both libraries.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
packages=${1:-${OUTPUT_DIR:-$root/target/packages}}
archives=("$packages"/redlinedb-*.tar.gz)
[[ ${#archives[@]} == 1 && -f ${archives[0]} ]] || { echo 'expected one native core archive' >&2; exit 1; }
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Regular files and directories only: a symlink, hard link or device entry
# could redirect an installer's writes.
bad_entries=$(tar -tvzf "${archives[0]}" | awk 'substr($1, 1, 1) != "-" && substr($1, 1, 1) != "d"')
[[ -z $bad_entries ]] || { printf 'archive holds non-regular entries:\n%s\n' "$bad_entries" >&2; exit 1; }

prefix="$work/install with spaces"
mkdir -p "$prefix"
tar -xzf "${archives[0]}" -C "$prefix"
abi_major=$(sed -n 's/^#define RLDB_ABI_MAJOR \([0-9][0-9]*\)$/\1/p' "$prefix/include/redlinedb.h")
[[ $abi_major =~ ^[0-9]+$ ]] || { echo 'packaged redlinedb.h lacks RLDB_ABI_MAJOR' >&2; exit 1; }
# The packaged sqlite3.h must be the shim over redlinedb.h, never upstream's.
grep -q '#include "redlinedb.h"' "$prefix/include/sqlite3.h" || { echo 'packaged sqlite3.h is not the RedlineDB shim' >&2; exit 1; }
[[ $(find "$prefix/include" -type f | wc -l) -eq 2 ]] || { echo 'include/ must hold only redlinedb.h and sqlite3.h' >&2; exit 1; }

case "$(uname -s)" in
  Darwin)
    library=libredlinedb.$abi_major.dylib unversioned=libredlinedb.dylib
    identity=$(otool -D "$prefix/lib/$library" | sed -n '2p')
    [[ $identity == "@rpath/$library" ]] || { echo "packaged dylib identity is '$identity', expected @rpath/$library" >&2; exit 1; }
    codesign --verify --strict "$prefix/lib/$library"
    rpath='@loader_path/../lib'
    native_libs=(-liconv -lSystem -lresolv)
    needed() { otool -L "$1" | awk 'NR > 1 {print $1}'; }
    ;;
  Linux)
    library=libredlinedb.so.$abi_major unversioned=libredlinedb.so
    soname=$(readelf -d "$prefix/lib/$library" | sed -n 's/.*(SONAME).*\[\(.*\)\]/\1/p')
    [[ $soname == "$library" ]] || { echo "packaged library soname is '$soname', expected $library" >&2; exit 1; }
    # The dynamic loader expands ORIGIN when the relocated consumer runs.
    # shellcheck disable=SC2016
    rpath='$ORIGIN/../lib'
    native_libs=(-ldl -lpthread -lm)
    needed() { readelf -d "$1" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p'; }
    ;;
  *) echo 'unsupported FFI test platform' >&2; exit 1 ;;
esac
[[ -f $prefix/lib/$library && -f $prefix/lib/libredlinedb.a ]] || { echo "archive lacks lib/$library or lib/libredlinedb.a" >&2; exit 1; }
# The unversioned development link is the installer's job, not the archive's.
[[ ! -e $prefix/lib/$unversioned ]] || { echo "archive ships lib/$unversioned" >&2; exit 1; }

cat > "$work/consumer.c" <<'C'
#include "sqlite3.h"
int main(void) {
    sqlite3 *db = 0;
    sqlite3_stmt *stmt = 0;
    if (sqlite3_open(":memory:", &db) != SQLITE_OK) return 1;
    if (sqlite3_prepare_v3(db, "SELECT 42, NULL", -1, SQLITE_PREPARE_PERSISTENT, &stmt, 0) != SQLITE_OK) return 2;
    if (sqlite3_step(stmt) != SQLITE_ROW || sqlite3_column_int64(stmt, 0) != 42) return 3;
    if (sqlite3_column_type(stmt, 1) != SQLITE_NULL || SQLITE_NULL != 5) return 4;
    sqlite3_finalize(stmt);
    return sqlite3_close(db);
}
C
cc "$work/consumer.c" -I "$prefix/include" "$prefix/lib/$library" -Wl,-rpath,"$rpath" -o "$prefix/bin/ffi-dynamic"
cc "$work/consumer.c" -I "$prefix/include" "$prefix/lib/libredlinedb.a" "${native_libs[@]}" -o "$prefix/bin/ffi-static"
needed "$prefix/bin/ffi-dynamic" | grep -Eq "(^|/)$library\$" || { echo "ffi-dynamic does not load $library" >&2; exit 1; }
# With the unversioned link the installer creates, -lredlinedb still binds the
# consumer to the ABI-major name, never to the link.
mkdir -p "$work/devlib"
ln -s "$prefix/lib/$library" "$work/devlib/$unversioned"
cc "$work/consumer.c" -I "$prefix/include" -L "$work/devlib" -lredlinedb -o "$work/ffi-devlink"
needed "$work/ffi-devlink" | grep -Eq "(^|/)$library\$" || { echo "-lredlinedb does not bind to $library" >&2; exit 1; }

mv "$prefix" "$work/relocated install"
relocated="$work/relocated install"
(cd "$work" && "$relocated/bin/ffi-dynamic" && "$relocated/bin/ffi-static")
printf 'Packaged dynamic/static FFI consumers passed after relocation (%s).\n' "$library"

# ABI_PROBE_OUT keeps the receipt; it must not be the packages directory,
# whose contents are uploaded as release assets.
bash "$root/scripts/compatibility/phase2-abi-probe.sh" \
  --library "$relocated/lib/$library" --static-archive "$relocated/lib/libredlinedb.a" \
  --out "${ABI_PROBE_OUT:-$work/abi-probe}"
printf 'Upstream-header ABI probe passed on the packaged libraries.\n'
