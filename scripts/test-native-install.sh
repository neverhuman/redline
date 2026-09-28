#!/usr/bin/env bash
# Install this platform's candidate core archive with the real install.sh, the
# way docs/install.md does, and use the installation. A file-transport curl
# serves the canonical installer and release URLs from the packages directory,
# and no development toolchain is on PATH while installing and querying.
# Afterwards a C consumer compiles against PREFIX/include, links with
# -lredlinedb through the development link and loads the library through the
# stable links (the macOS @rpath and Linux soname lookups through symlinks).
#
#   scripts/test-native-install.sh [packages dir]
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
packages=$(cd "${1:-${OUTPUT_DIR:-$root/target/packages}}" && pwd)
# shellcheck source=scripts/release/test-shims.sh
. "$root/scripts/release/test-shims.sh"
die() { printf 'native install: %s\n' "$*" >&2; exit 1; }
archives=("$packages"/redlinedb-v*.tar.gz)
[[ ${#archives[@]} == 1 && -f ${archives[0]} ]] || die "expected one native core archive in $packages"
tag=$(tar -xzOf "${archives[0]}" ./share/redlinedb/VERSION)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
write_no_toolchain "$work/no-toolchain"
write_release_transport "$work/transport"
export RELEASE_PACKAGES=$packages RELEASE_TAG=$tag RELEASE_INSTALLER=$root/install.sh
binary_path="$work/no-toolchain:$work/transport:$PATH"
prefix="$work/native prefix"
base=$prefix/lib/redlinedb
case "$(uname -s)" in
  Darwin) library=libredlinedb.5.dylib devlink=libredlinedb.dylib ;;
  *) library=libredlinedb.so.5 devlink=libredlinedb.so ;;
esac

# The pinned install from the published installer URL.
PATH=$binary_path bash -c 'curl -fsSL "https://raw.githubusercontent.com/neverhuman/redline/$RELEASE_TAG/install.sh" | VERSION="$RELEASE_TAG" PREFIX="$1" bash' \
  _ "$prefix" || die "installing $tag failed"
[[ $(readlink "$base/current") == "versions/$tag" ]] || die "current is not versions/$tag"
for path in bin/redlinedb bin/redlinedb-server "lib/$library" "lib/$devlink" lib/libredlinedb.a include/redlinedb.h include/sqlite3.h; do
  [[ -L $prefix/$path && -e $prefix/$path ]] || die "$path is not a working stable link"
done
[[ $prefix/lib/$devlink -ef $base/versions/$tag/lib/$library ]] || die "lib/$devlink does not reach $library"
[[ ! -e $prefix/bin/sqlite3 ]] || die 'the installer created bin/sqlite3'
[[ -z $(cd "$base/versions/$tag" && find . ! -type f ! -type d ! -path "./lib/$devlink") ]] ||
  die 'the installed version holds links other than the development link'

# First query and a database that survives close and reopen.
answer=$(cd "$work" && PATH=$binary_path "$prefix/bin/redlinedb" -batch :memory: 'SELECT 1;')
[[ $answer == 1 ]] || die "SELECT 1 printed '$answer'"
database="$work/data dir/persistent.redline"
mkdir -p "${database%/*}"
PATH=$binary_path "$prefix/bin/redlinedb" -batch -bail "$database" 'CREATE TABLE t(n INTEGER); INSERT INTO t VALUES (42);'
answer=$(PATH=$binary_path "$prefix/bin/redlinedb" -batch -bail "$database" 'SELECT n FROM t;')
[[ $answer == 42 ]] || die "the reopened database returned '$answer'"

# The default (latest) install of the same release keeps it active; with no
# earlier version there is nothing to roll back to.
PATH=$binary_path VERSION='' PREFIX=$prefix bash "$root/install.sh" || die 'reinstalling through releases/latest failed'
[[ $(readlink "$base/current") == "versions/$tag" && ! -e $base/previous ]] || die 'a reinstall changed current or previous'
if PATH=$binary_path REDLINEDB_ROLLBACK=1 PREFIX=$prefix bash "$root/install.sh" 2> "$work/rollback.log"; then
  die 'rollback without a previous version succeeded'
fi
grep -q 'no previous version' "$work/rollback.log" || die "unexpected rollback refusal: $(cat "$work/rollback.log")"
printf 'Native install of %s: stable links, SELECT 1, persistence, reinstall and rollback refusal passed.\n' "$tag"

# A C program built against the installation, as its users build one.
if [[ ${NATIVE_INSTALL_FFI:-1} == 0 ]]; then
  printf 'NATIVE_INSTALL_FFI=0: the installed C library was not linked.\n'
  exit 0
fi
cat > "$work/consumer.c" <<'C'
#include <sqlite3.h>
int main(void) {
    sqlite3 *db = 0;
    sqlite3_stmt *stmt = 0;
    if (sqlite3_open(":memory:", &db) != SQLITE_OK) return 1;
    if (sqlite3_prepare_v2(db, "SELECT 42", -1, &stmt, 0) != SQLITE_OK) return 2;
    if (sqlite3_step(stmt) != SQLITE_ROW || sqlite3_column_int64(stmt, 0) != 42) return 3;
    sqlite3_finalize(stmt);
    return sqlite3_close(db);
}
C
cc "$work/consumer.c" -I "$prefix/include" -L "$prefix/lib" -lredlinedb -Wl,-rpath,"$prefix/lib" -o "$work/consumer"
case "$(uname -s)" in
  Darwin) loads=$(otool -L "$work/consumer" | awk 'NR > 1 {print $1}') expected="@rpath/$library" ;;
  *) loads=$(readelf -d "$work/consumer" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p') expected=$library ;;
esac
grep -qxF "$expected" <<< "$loads" || die "-lredlinedb bound the consumer to '$loads', not $expected"
(cd / && "$work/consumer") || die 'the consumer failed against the installed library'
printf 'C consumer linked through %s/lib/%s and loaded %s through the stable links.\n' "$prefix" "$devlink" "$expected"
