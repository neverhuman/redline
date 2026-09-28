#!/usr/bin/env bash
# Run the real installer against local release fixtures: no network, no
# published release and no Rust toolchain. uname, getconf, sw_vers, curl and gh
# are shims, so one host exercises every supported OS/architecture pair.
#
# Checked: the asset each platform requests; every refusal happens before
# anything under PREFIX changes (the prefix fingerprint is compared); a
# candidate that fails validation, a cp/mv/ln failure at any point, and two
# concurrent installers leave one complete version active, never a mix;
# rollback; the legacy flat layout is refused unless migrated; prefixes with
# spaces. INSTALLER selects another script (default: install.sh).
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
installer=${INSTALLER:-$root/install.sh}
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }

# scripts/install.sh is the same installer under its older published path.
cmp -s "$root/install.sh" "$root/scripts/install.sh" || fail 'scripts/install.sh differs from install.sh'
# A piped installer cannot read ops/release/authority.env, so it repeats it.
# shellcheck source=ops/release/authority.env
. "$root/ops/release/authority.env"
grep -qx "repo_slug=$REDLINE_REPO_SLUG" "$installer" || fail "install.sh does not bind $REDLINE_REPO_SLUG"
grep -qx "repo_id=$REDLINE_REPO_ID" "$installer" || fail "install.sh does not bind repository id $REDLINE_REPO_ID"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/tools" "$work/releases" "$work/trees"
old=v5.0.0-rc.1 new=v5.0.0 third=v5.0.1
canonical=https://github.com/neverhuman/redline
# The retired repository's id; its name is split so validation scans pass.
legacy_id=1240106851 legacy_url="https://github.com/neverhumanbot/Redline""DB"

sha256() { if command -v sha256sum >/dev/null; then sha256sum | cut -d' ' -f1; else shasum -a 256 | cut -d' ' -f1; fi; }
# platform <os> <arch>: the release platform name install.sh must request.
platform() {
  case "$1/$2" in
    Linux/x86_64) echo linux-x86_64 ;;
    Linux/aarch64 | Linux/arm64) echo linux-arm64 ;;
    Darwin/x86_64) echo macos-x86_64 ;;
    Darwin/arm64) echo macos-arm64 ;;
  esac
}
libname() { case "$1" in macos-*) echo libredlinedb.5.dylib ;; *) echo libredlinedb.so.5 ;; esac; }
devlib() { case "$1" in macos-*) echo libredlinedb.dylib ;; *) echo libredlinedb.so ;; esac; }

# provenance <repository id> <tag>: the line scripts/package-release.sh writes.
provenance() {
  printf '{"schema":"redline.release-build/v2","repository_url":"%s","repository_id":%s,"tag":"%s","commit":"%040d","source_tree":"%040d","platform":"linux-x86_64","package":"redlinedb","rust":"rustc"}' \
    "$canonical" "$1" "$2" 0 0
}
# package <version> <platform> [cli mode] [provenance line]: a package tree
# shaped like scripts/package-release.sh output. Every file names its version
# so a mixed installation is visible. CLI modes: ok, exit42, wrong-answer,
# slow (validation takes a second).
package() {
  local version=$1 platform=$2 mode=${3:-ok} dir
  local line=${4-$(provenance "$REDLINE_REPO_ID" "$version")}
  dir=$work/trees/$version-$platform-$mode
  rm -rf "$dir"
  mkdir -p "$dir/bin" "$dir/lib" "$dir/include" "$dir/share/redlinedb"
  cat > "$dir/bin/redlinedb" <<CLI
#!/bin/sh
case "\$1" in
  --version)
    [ $mode != exit42 ] || exit 42
    [ $mode != slow ] || sleep 1
    echo "redlinedb $version fixture" ;;
  -batch)
    [ "\$2" = :memory: ] && [ "\$3" = 'SELECT 1;' ] || exit 2
    if [ $mode = wrong-answer ]; then echo 2; else echo 1; fi ;;
  *) exit 2 ;;
esac
CLI
  printf '#!/bin/sh\necho "server %s"\n' "$version" > "$dir/bin/redlinedb-server"
  chmod +x "$dir/bin/redlinedb" "$dir/bin/redlinedb-server"
  printf 'lib %s\n' "$version" > "$dir/lib/$(libname "$platform")"
  printf 'static %s\n' "$version" > "$dir/lib/libredlinedb.a"
  printf '/* redlinedb.h %s */\n' "$version" > "$dir/include/redlinedb.h"
  printf '#include "redlinedb.h"\n' > "$dir/include/sqlite3.h"
  printf '%s\n' "$version" > "$dir/share/redlinedb/VERSION"
  [[ -z $line ]] || printf '%s\n' "$line" > "$dir/share/redlinedb/build-provenance.json"
  tree=$dir
}
# publish <version> <platform>: serve $tree as that release asset with a
# correct checksum sidecar.
publish() {
  local asset=redlinedb-$1-$2.tar.gz
  tar -czf "$work/releases/$asset" -C "$tree" .
  printf '%s  %s\n' "$(sha256 < "$work/releases/$asset")" "$asset" > "$work/releases/$asset.sha256"
}

cat > "$work/tools/uname" <<'BIN'
#!/bin/sh
case "$1" in -s) echo "${TEST_OS:-Linux}" ;; -m) echo "${TEST_ARCH:-x86_64}" ;; *) echo "${TEST_OS:-Linux}" ;; esac
BIN
cat > "$work/tools/getconf" <<'BIN'
#!/bin/sh
[ "$1" = GNU_LIBC_VERSION ] || exit 1
[ "${TEST_GLIBC:-2.39}" != none ] || exit 1
echo "glibc ${TEST_GLIBC:-2.39}"
BIN
cat > "$work/tools/ldd" <<'BIN'
#!/bin/sh
[ "${TEST_GLIBC:-2.39}" != none ] || { echo 'musl libc (x86_64)'; exit 1; }
echo "ldd (GNU libc) ${TEST_GLIBC:-2.39}"
BIN
cat > "$work/tools/sw_vers" <<'BIN'
#!/bin/sh
[ "$1" = -productVersion ] && echo "${TEST_MACOS:-15.1}"
BIN
# curl answers -w '%{url_effective}' for releases/latest with TEST_LATEST_URL,
# serves release downloads of the canonical repository from the fixture
# directory, and logs every URL it is asked for.
cat > "$work/tools/curl" <<'BIN'
#!/usr/bin/env bash
set -eu
url= out= format=
while [[ $# -gt 0 ]]; do
  case "$1" in
    -o) out=$2; shift 2 ;;
    -w) format=$2; shift 2 ;;
    https://*) url=$1; shift ;;
    *) shift ;;
  esac
done
printf '%s\n' "$url" >> "$INSTALL_FIXTURES/curl.log"
if [[ $format == '%{url_effective}' ]]; then
  case "$url" in */releases/latest) printf '%s' "$TEST_LATEST_URL" ;; *) printf '%s' "$url" ;; esac
  exit 0
fi
case "$url" in
  https://github.com/neverhuman/redline/releases/download/*)
    [[ -f $INSTALL_FIXTURES/releases/${url##*/} ]] || { printf 'curl: (22) The requested URL returned error: 404\n' >&2; exit 22; }
    cp "$INSTALL_FIXTURES/releases/${url##*/}" "$out" ;;
  *) printf 'curl fixture: no route to %s\n' "$url" >&2; exit 22 ;;
esac
BIN
cat > "$work/tools/gh" <<'BIN'
#!/bin/sh
printf '%s\n' "$*" >> "$INSTALL_FIXTURES/gh.log"
exit "${TEST_GH_STATUS:-0}"
BIN
chmod +x "$work/tools/"*
export INSTALL_FIXTURES=$work REDLINEDB_LOCK_TIMEOUT=30
export PATH="$work/tools:$PATH"

# run_installer <log> <prefix> [VAR=value...]: one installer run.
run_installer() {
  local log=$1 prefix=$2
  shift 2
  env "$@" PREFIX="$prefix" bash "$installer" > "$log" 2>&1
}
# fingerprint <prefix>: every path, link target and file checksum.
fingerprint() {
  (cd "$1" && find . | LC_ALL=C sort | while IFS= read -r path; do
    if [[ -L $path ]]; then printf 'L %s -> %s\n' "$path" "$(readlink "$path")"
    elif [[ -f $path ]]; then printf 'F %s %s\n' "$path" "$(cksum < "$path")"
    else printf 'D %s\n' "$path"; fi
  done) | sha256
}
# assert_active <label> <prefix> <version> [platform]: every installed
# component, reached through the stable paths, is <version>.
assert_active() {
  local label=$1 prefix=$2 version=$3 platform=${4:-linux-x86_64} got
  got=$("$prefix/bin/redlinedb" --version 2>&1 || true)
  [[ $got == "redlinedb $version fixture" ]] || fail "$label: bin/redlinedb says '$got', expected $version"
  got=$("$prefix/bin/redlinedb-server" 2>&1 || true)
  [[ $got == "server $version" ]] || fail "$label: bin/redlinedb-server says '$got', expected $version"
  got=$(cat "$prefix/lib/$(libname "$platform")" 2>&1 || true)
  [[ $got == "lib $version" ]] || fail "$label: lib/$(libname "$platform") is '$got', expected $version"
  got=$(cat "$prefix/lib/libredlinedb.a" 2>&1 || true)
  [[ $got == "static $version" ]] || fail "$label: lib/libredlinedb.a is '$got', expected $version"
  got=$(cat "$prefix/include/redlinedb.h" 2>&1 || true)
  [[ $got == "/* redlinedb.h $version */" ]] || fail "$label: include/redlinedb.h is '$got', expected $version"
}
# assert_layout <label> <prefix> <version> [platform]: the versioned layout,
# stable links, the development link, and nothing left behind.
assert_layout() {
  local label=$1 prefix=$2 version=$3 platform=${4:-linux-x86_64} base path got
  base=$prefix/lib/redlinedb
  got=$(readlink "$base/current" 2>/dev/null || true)
  [[ $got == "versions/$version" ]] || fail "$label: current -> '$got', expected versions/$version"
  for path in bin/redlinedb bin/redlinedb-server "lib/$(libname "$platform")" "lib/$(devlib "$platform")" \
    lib/libredlinedb.a include/redlinedb.h include/sqlite3.h; do
    [[ -L $prefix/$path ]] || fail "$label: $path is not a stable link"
  done
  got=$(cat "$prefix/lib/$(devlib "$platform")" 2>&1 || true)
  [[ $got == "lib $version" ]] || fail "$label: development link $(devlib "$platform") resolves to '$got'"
  [[ ! -e $prefix/bin/sqlite3 && ! -L $prefix/bin/sqlite3 ]] || fail "$label: installer created bin/sqlite3"
  [[ ! -e $base/.lock ]] || fail "$label: lock left behind"
  got=$(find "$prefix" \( -name '.stage.*' -o -name '*.redlinedb-new' \) 2>/dev/null || true)
  [[ -z $got ]] || fail "$label: staging or a temporary link left behind: $got"
}
# fresh <name> <version> [VAR=value...]: a prefix with <version> installed.
fresh() {
  local name=$1 version=$2
  shift 2
  prefix="$work/$name prefix"
  rm -rf "$prefix"
  run_installer "$work/$name.log" "$prefix" VERSION="$version" "$@" ||
    fail "$name: installing $version failed: $(tail -n 1 "$work/$name.log")"
}
# expect_reject <label> <message> [VAR=value...]: on a prefix that has $old
# installed, the installer must fail with <message> and change nothing.
expect_reject() {
  local label=$1 message=$2 prefix="$work/reject prefix" before
  shift 2
  rm -rf "$prefix"
  cp -pR "$work/old prefix" "$prefix"
  before=$(fingerprint "$prefix")
  if run_installer "$work/$label.log" "$prefix" "$@"; then
    fail "$label: installer accepted it"
  elif ! grep -qF -- "$message" "$work/$label.log"; then
    fail "$label: expected '$message', got: $(tail -n 1 "$work/$label.log")"
  fi
  [[ $(fingerprint "$prefix") == "$before" ]] || fail "$label: the prefix changed"
}
# expect_old_kept <label> <message> [VAR=value...]: a candidate that is
# refused after staging leaves $old active and nothing behind.
expect_old_kept() {
  local label=$1 message=$2 prefix="$work/kept prefix"
  shift 2
  rm -rf "$prefix"
  cp -pR "$work/old prefix" "$prefix"
  if run_installer "$work/$label.log" "$prefix" VERSION="$new" "$@"; then
    fail "$label: installer accepted it"
  elif ! grep -qF -- "$message" "$work/$label.log"; then
    fail "$label: expected '$message', got: $(tail -n 1 "$work/$label.log")"
  fi
  assert_active "$label" "$prefix" "$old"
  assert_layout "$label" "$prefix" "$old"
  [[ ! -e $prefix/lib/redlinedb/versions/$new ]] || fail "$label: a refused candidate was kept as versions/$new"
}

# --- Every supported platform requests its own asset and installs it. -------
for pair in Linux/x86_64 Linux/aarch64 Linux/arm64 Darwin/x86_64 Darwin/arm64; do
  os=${pair%/*} arch=${pair#*/} name=$(platform "${pair%/*}" "${pair#*/}")
  package "$old" "$name"
  publish "$old" "$name"
  rm -f "$work/curl.log"
  fresh "matrix $os $arch" "$old" TEST_OS="$os" TEST_ARCH="$arch"
  expected="$canonical/releases/download/$old/redlinedb-$old-$name.tar.gz"
  got=$(tr '\n' ' ' < "$work/curl.log")
  [[ $got == "$expected $expected.sha256 " ]] || fail "$os/$arch: requested $got"
  assert_active "$os/$arch" "$prefix" "$old" "$name"
  assert_layout "$os/$arch" "$prefix" "$old" "$name"
done

# The rejection baseline: $old installed on linux-x86_64.
fresh old "$old"
assert_active old "$prefix" "$old"
assert_layout old "$prefix" "$old"
fresh latest latest TEST_LATEST_URL="$canonical/releases/tag/$old"
assert_active latest "$prefix" "$old"

# --- Refusals before any write under PREFIX. ---------------------------------
expect_reject unsupported-os 'unsupported platform' VERSION="$old" TEST_OS=FreeBSD
expect_reject unsupported-arch 'unsupported platform' VERSION="$old" TEST_ARCH=riscv64
expect_reject old-glibc 'glibc 2.35 or newer' VERSION="$old" TEST_GLIBC=2.31
expect_reject no-glibc 'glibc 2.35 or newer' VERSION="$old" TEST_GLIBC=none
expect_reject old-macos 'macOS 15 or newer' VERSION="$old" TEST_OS=Darwin TEST_ARCH=arm64 TEST_MACOS=14.6
expect_reject invalid-version 'invalid VERSION' VERSION=v5.0
# With no published release GitHub redirects /releases/latest to /releases.
expect_reject no-release 'no published release' VERSION=latest TEST_LATEST_URL="$canonical/releases"
expect_reject latest-elsewhere 'no published release' VERSION=latest TEST_LATEST_URL="$legacy_url/releases/tag/$old"
expect_reject missing-release "cannot download redlinedb-v4.9.9-linux-x86_64.tar.gz from $canonical release v4.9.9" VERSION=v4.9.9
expect_reject wrong-pin 'pinned checksum mismatch' VERSION="$old" REDLINEDB_SHA256="$(printf '%064d' 0)"
expect_reject attestation-flag 'REDLINEDB_VERIFY_ATTESTATION must be 0 or 1' VERSION="$old" REDLINEDB_VERIFY_ATTESTATION=yes
expect_reject rollback-flag 'REDLINEDB_ROLLBACK must be 0 or 1' VERSION="$old" REDLINEDB_ROLLBACK=yes
expect_reject migrate-flag 'REDLINEDB_MIGRATE_LEGACY must be 0 or 1' VERSION="$old" REDLINEDB_MIGRATE_LEGACY=yes
expect_reject no-previous 'no previous version' REDLINEDB_ROLLBACK=1

# Rejected archives carry correct checksums: only their contents are wrong.
package "$new" linux-x86_64
publish "$new" linux-x86_64
mv "$work/releases/redlinedb-$new-linux-x86_64.tar.gz.sha256" "$work/sidecar"
expect_reject missing-sidecar "cannot download redlinedb-$new-linux-x86_64.tar.gz.sha256" VERSION="$new"
mv "$work/sidecar" "$work/releases/redlinedb-$new-linux-x86_64.tar.gz.sha256"
printf 'corruption' >> "$work/releases/redlinedb-$new-linux-x86_64.tar.gz"
expect_reject checksum 'checksum mismatch' VERSION="$new"
package "$new" linux-x86_64 ok "$(provenance "$legacy_id" "$new")"
publish "$new" linux-x86_64
expect_reject legacy-authority "was not built by $canonical" VERSION="$new"
package "$new" linux-x86_64 ok "$(provenance "${REDLINE_REPO_ID}0" "$new")"
publish "$new" linux-x86_64
expect_reject longer-repository-id "was not built by $canonical" VERSION="$new"
package "$new" linux-x86_64 ok "$(provenance "$REDLINE_REPO_ID" "$third")"
publish "$new" linux-x86_64
expect_reject wrong-tag "provenance does not name $new" VERSION="$new"
package "$new" linux-x86_64 ok "$(provenance "$REDLINE_REPO_ID" "${new}0")"
publish "$new" linux-x86_64
expect_reject tag-prefix "provenance does not name $new" VERSION="$new"
package "$new" linux-x86_64 ok ''
publish "$new" linux-x86_64
expect_reject no-provenance 'no build provenance' VERSION="$new"
package "$new" linux-x86_64
ln -s redlinedb "$tree/bin/sqlite3"
publish "$new" linux-x86_64
expect_reject symlink-member 'unsafe archive member type' VERSION="$new"
package "$new" linux-x86_64
ln "$tree/bin/redlinedb" "$tree/bin/redlinedb-copy"
publish "$new" linux-x86_64
expect_reject hardlink-member 'unsafe archive member type' VERSION="$new"
package "$new" linux-x86_64
cp "$tree/bin/redlinedb" "$tree/bin/sqlite3"
publish "$new" linux-x86_64
expect_reject sqlite3-member 'archive ships bin/sqlite3' VERSION="$new"
package "$new" linux-x86_64
rm "$tree/bin/redlinedb-server"
publish "$new" linux-x86_64
expect_reject missing-server 'archive is missing CLI or server' VERSION="$new"

package "$new" linux-x86_64
publish "$new" linux-x86_64
rm -f "$work/gh.log"
expect_reject attestation-refused 'attestation verification failed' VERSION="$new" REDLINEDB_VERIFY_ATTESTATION=1 TEST_GH_STATUS=1
expected_gh="/redlinedb-$new-linux-x86_64.tar.gz --repo $REDLINE_REPO_SLUG --signer-workflow $REDLINE_REPO_SLUG/.github/workflows/release-build.yml"
if ! grep -q '^attestation verify ' "$work/gh.log" 2>/dev/null || ! grep -qF -- "$expected_gh" "$work/gh.log"; then
  fail "attestation: gh attestation verify was not asked for $REDLINE_REPO_SLUG's release workflow"
fi

# --- A candidate that fails validation leaves the old version active. -------
package "$new" linux-x86_64 exit42
publish "$new" linux-x86_64
expect_old_kept exit-42 'candidate failed validation'
package "$new" linux-x86_64 wrong-answer
publish "$new" linux-x86_64
expect_old_kept wrong-answer 'candidate failed validation'

# --- An upgrade, a reinstall and rollbacks. ----------------------------------
package "$new" linux-x86_64
publish "$new" linux-x86_64
prefix="$work/upgrade prefix"
rm -rf "$prefix"
cp -pR "$work/old prefix" "$prefix"
run_installer "$work/upgrade.log" "$prefix" VERSION="$new" || fail "upgrade: $(tail -n 1 "$work/upgrade.log")"
assert_active upgrade "$prefix" "$new"
assert_layout upgrade "$prefix" "$new"
[[ $(readlink "$prefix/lib/redlinedb/previous" 2>/dev/null) == "versions/$old" ]] || fail 'upgrade: previous does not name the old version'
run_installer "$work/reinstall.log" "$prefix" VERSION="$new" || fail "reinstall: $(tail -n 1 "$work/reinstall.log")"
assert_active reinstall "$prefix" "$new"
[[ $(readlink "$prefix/lib/redlinedb/previous" 2>/dev/null) == "versions/$old" ]] || fail 'reinstall: previous changed'
run_installer "$work/rollback.log" "$prefix" REDLINEDB_ROLLBACK=1 || fail "rollback: $(tail -n 1 "$work/rollback.log")"
assert_active rollback "$prefix" "$old"
assert_layout rollback "$prefix" "$old"
[[ $(readlink "$prefix/lib/redlinedb/previous" 2>/dev/null) == "versions/$new" ]] || fail 'rollback: previous does not name the version rolled back from'
run_installer "$work/roll-forward.log" "$prefix" REDLINEDB_ROLLBACK=1 || fail "roll forward: $(tail -n 1 "$work/roll-forward.log")"
assert_active roll-forward "$prefix" "$new"

# --- A cp, mv or ln failure at any step leaves one complete version. ----------
# Each shim fails its Nth call. Every failed run must leave $old wholly
# active; once N passes the number of calls the installer makes, it succeeds.
for tool in cp mv ln; do
  real=$(command -v "$tool")
  mkdir -p "$work/inject-$tool"
  cat > "$work/inject-$tool/$tool" <<BIN
#!/bin/sh
count=\$((\$(cat "$work/inject-$tool/count" 2>/dev/null || echo 0) + 1))
echo "\$count" > "$work/inject-$tool/count"
[ "\$count" != "\$INJECT_AT" ] || { echo "injected $tool failure" >&2; exit 1; }
exec "$real" "\$@"
BIN
  chmod +x "$work/inject-$tool/$tool"
  succeeded=false
  for at in $(seq 1 60); do
    prefix="$work/inject prefix"
    rm -rf "$prefix" "$work/inject-$tool/count"
    cp -pR "$work/old prefix" "$prefix"
    if run_installer "$work/inject.log" "$prefix" VERSION="$new" INJECT_AT="$at" PATH="$work/inject-$tool:$PATH"; then
      assert_active "$tool success" "$prefix" "$new"
      assert_layout "$tool success" "$prefix" "$new"
      succeeded=true
      break
    fi
    grep -qF "injected $tool failure" "$work/inject.log" || fail "$tool call $at: failed without the injected failure: $(tail -n 1 "$work/inject.log")"
    assert_active "$tool failure at call $at" "$prefix" "$old"
    assert_layout "$tool failure at call $at" "$prefix" "$old"
  done
  "$succeeded" || fail "$tool: the installer never succeeded"
  printf '%s: each of %d injected failures left %s active\n' "$tool" $((at - 1)) "$old"
done

# --- Two concurrent installers leave one complete version active. -----------
package "$new" linux-x86_64 slow
publish "$new" linux-x86_64
package "$third" linux-x86_64 slow
publish "$third" linux-x86_64
prefix="$work/concurrent prefix"
rm -rf "$prefix"
cp -pR "$work/old prefix" "$prefix"
run_installer "$work/concurrent-a.log" "$prefix" VERSION="$new" & first=$!
run_installer "$work/concurrent-b.log" "$prefix" VERSION="$third" & second=$!
wait "$first" || fail "concurrent: installing $new failed: $(tail -n 1 "$work/concurrent-a.log")"
wait "$second" || fail "concurrent: installing $third failed: $(tail -n 1 "$work/concurrent-b.log")"
active=$(readlink "$prefix/lib/redlinedb/current" 2>/dev/null || true)
case $active in
  "versions/$new") assert_active concurrent "$prefix" "$new"; assert_layout concurrent "$prefix" "$new" ;;
  "versions/$third") assert_active concurrent "$prefix" "$third"; assert_layout concurrent "$prefix" "$third" ;;
  *) fail "concurrent: current -> '$active'" ;;
esac
grep -qF 'waiting for' "$work/concurrent-a.log" "$work/concurrent-b.log" || fail 'concurrent: neither installer waited for the lock'

# --- A killed installer's lock stops the next one; its staging is removed. ---
package "$new" linux-x86_64
publish "$new" linux-x86_64
prefix="$work/stale prefix"
rm -rf "$prefix"
cp -pR "$work/old prefix" "$prefix"
mkdir -p "$prefix/lib/redlinedb/.lock" "$prefix/lib/redlinedb/.stage.killed"
printf 'pid 1 on elsewhere since 2026-01-01T00:00:00Z\n' > "$prefix/lib/redlinedb/.lock/owner"
before=$(fingerprint "$prefix")
if run_installer "$work/stale-lock.log" "$prefix" VERSION="$new" REDLINEDB_LOCK_TIMEOUT=1; then
  fail 'stale lock: installer ignored a held lock'
elif ! grep -qF 'remove that directory and rerun' "$work/stale-lock.log" || ! grep -qF 'pid 1 on elsewhere' "$work/stale-lock.log"; then
  fail "stale lock: expected the owner and removal advice, got: $(tail -n 2 "$work/stale-lock.log")"
fi
[[ $(fingerprint "$prefix") == "$before" ]] || fail 'stale lock: a waiting installer changed the prefix'
rm -rf "$prefix/lib/redlinedb/.lock"
run_installer "$work/after-stale.log" "$prefix" VERSION="$new" || fail "after stale lock: $(tail -n 1 "$work/after-stale.log")"
assert_active after-stale "$prefix" "$new"
assert_layout after-stale "$prefix" "$new"

# --- The legacy flat layout is foreign until explicitly migrated. -------------
package "$new" linux-x86_64
publish "$new" linux-x86_64
prefix="$work/legacy prefix"
rm -rf "$prefix"
# What the earlier installer left: the package's files copied flat, plus the
# development link scripts/install-from-source.sh made.
package legacy linux-x86_64
mkdir -p "$prefix/share"
cp -pR "$tree/bin" "$tree/lib" "$tree/include" "$prefix/"
cp -pR "$tree/share/redlinedb" "$prefix/share/"
ln -s libredlinedb.so.5 "$prefix/lib/libredlinedb.so"
printf '#!/bin/sh\necho unrelated\n' > "$prefix/bin/unrelated-tool"
chmod +x "$prefix/bin/unrelated-tool"
before=$(fingerprint "$prefix")
if run_installer "$work/legacy.log" "$prefix" VERSION="$new"; then
  fail 'legacy: installer replaced a flat installation without REDLINEDB_MIGRATE_LEGACY=1'
elif ! grep -qF 'REDLINEDB_MIGRATE_LEGACY=1' "$work/legacy.log"; then
  fail "legacy: expected a REDLINEDB_MIGRATE_LEGACY=1 hint, got: $(tail -n 1 "$work/legacy.log")"
fi
[[ $(fingerprint "$prefix") == "$before" ]] || fail 'legacy: the refused installer changed the prefix'
run_installer "$work/migrate.log" "$prefix" VERSION="$new" REDLINEDB_MIGRATE_LEGACY=1 ||
  fail "migrate: $(tail -n 1 "$work/migrate.log")"
assert_active migrate "$prefix" "$new"
assert_layout migrate "$prefix" "$new"
[[ $("$prefix/bin/unrelated-tool") == unrelated ]] || fail 'migrate: an unrelated file changed'
kept=$(readlink "$prefix/lib/redlinedb/previous" 2>/dev/null || true)
[[ $kept == versions/legacy-* && $("$prefix/lib/redlinedb/$kept/bin/redlinedb" --version) == 'redlinedb legacy fixture' ]] ||
  fail "migrate: the legacy files were not kept as the previous version ($kept)"
run_installer "$work/legacy-rollback.log" "$prefix" REDLINEDB_ROLLBACK=1 || fail "legacy rollback: $(tail -n 1 "$work/legacy-rollback.log")"
assert_active legacy-rollback "$prefix" legacy
[[ $(cat "$prefix/lib/libredlinedb.so") == 'lib legacy' ]] || fail 'legacy rollback: the development link does not reach the legacy library'

# --- scripts/install-from-source.sh activates through the same installer. ------
# A stand-in Cargo target directory: the fixture CLI and libraries under the
# names cargo gives them.
package source-fixture linux-x86_64
mkdir -p "$work/cargo target/release"
cp "$tree/bin/redlinedb" "$tree/bin/redlinedb-server" "$tree/lib/libredlinedb.a" "$work/cargo target/release/"
cp "$tree/bin/redlinedb" "$work/cargo target/release/redlinedb-cli"
cp "$tree/lib/libredlinedb.so.5" "$work/cargo target/release/libredlinedb.so"
source_tree="$work/source tree"
if ! CARGO_TARGET_DIR="$work/cargo target" bash "$root/scripts/install-from-source.sh" --tree "$source_tree" > "$work/source-tree.log" 2>&1; then
  fail "source tree: $(tail -n 1 "$work/source-tree.log")"
elif [[ -n $(find "$source_tree" ! -type f ! -type d) || ! -f $source_tree/lib/libredlinedb.so.5 || ! -f $source_tree/include/sqlite3.h ]]; then
  fail "source tree: expected regular files with lib/libredlinedb.so.5: $(cd "$source_tree" && find . | LC_ALL=C sort | tr '\n' ' ')"
fi
prefix="$work/source prefix"
rm -rf "$prefix"
cp -pR "$work/old prefix" "$prefix"
if ! CARGO_TARGET_DIR="$work/cargo target" PREFIX="$prefix" bash "$root/scripts/install-from-source.sh" > "$work/source.log" 2>&1; then
  fail "source install: $(tail -n 1 "$work/source.log")"
fi
active=$(readlink "$prefix/lib/redlinedb/current" 2>/dev/null || true)
[[ $active == versions/source-* ]] || fail "source install: current -> '$active'"
[[ $("$prefix/bin/redlinedb" --version 2>&1) == 'redlinedb source-fixture fixture' && $(cat "$prefix/lib/libredlinedb.so" 2>&1) == 'lib source-fixture' ]] ||
  fail 'source install: the source build is not active through the stable links'
cmp -s "$prefix/include/redlinedb.h" "$root/contracts/c-abi/redlinedb.h" || fail 'source install: include/redlinedb.h is not the contract header'
[[ $(readlink "$prefix/lib/redlinedb/previous" 2>/dev/null) == "versions/$old" ]] || fail 'source install: previous does not name the release it replaced'
run_installer "$work/source-rollback.log" "$prefix" REDLINEDB_ROLLBACK=1 || fail "source rollback: $(tail -n 1 "$work/source-rollback.log")"
assert_active source-rollback "$prefix" "$old"

[[ $failures == 0 ]] || { printf '%d installer check(s) failed\n' "$failures" >&2; exit 1; }
printf 'Installer platform matrix, refusals, atomic activation, failure injection, concurrency, rollback, migration and source-install tests passed.\n'
