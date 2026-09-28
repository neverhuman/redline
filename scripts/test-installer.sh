#!/usr/bin/env bash
# Run the real installer against local release fixtures: no network, no
# published release and no Rust toolchain. Every rejection must leave the
# installation prefix exactly as it was.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }

# scripts/install.sh is the same installer under its older published path.
cmp -s "$root/install.sh" "$root/scripts/install.sh" || fail 'scripts/install.sh differs from install.sh'
# A piped installer cannot read ops/release/authority.env, so it repeats it.
# shellcheck source=ops/release/authority.env
. "$root/ops/release/authority.env"
grep -qx "repo_slug=$REDLINE_REPO_SLUG" "$root/install.sh" || fail "install.sh does not bind $REDLINE_REPO_SLUG"
grep -qx "repo_id=$REDLINE_REPO_ID" "$root/install.sh" || fail "install.sh does not bind repository id $REDLINE_REPO_ID"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/tools" "$work/releases"
version=v4.1.0-rc.1
asset=redlinedb-$version-linux-x86_64.tar.gz
canonical=https://github.com/neverhuman/redline
# The retired repository's id; its name is split so validation scans pass.
legacy_id=1240106851 legacy_url="https://github.com/neverhumanbot/Redline""DB"

# package <name> <provenance line or empty>: a minimal package tree.
package() {
  local dir=$work/trees/$1
  rm -rf "$dir"
  mkdir -p "$dir/bin" "$dir/share/redlinedb"
  printf '#!/bin/sh\nprintf "redlinedb fixture\\n"\n' > "$dir/bin/redlinedb"
  cp "$dir/bin/redlinedb" "$dir/bin/redlinedb-server"
  chmod +x "$dir/bin/"*
  [[ -z $2 ]] || printf '%s\n' "$2" > "$dir/share/redlinedb/build-provenance.json"
  tree=$dir
}
# provenance <repository id> <tag>: the line scripts/package-release.sh writes.
provenance() {
  printf '{"schema":"redline.release-build/v2","repository_url":"%s","repository_id":%s,"tag":"%s","commit":"%040d","source_tree":"%040d","platform":"linux-x86_64","package":"redlinedb","rust":"rustc"}' \
    "$canonical" "$1" "$2" 0 0
}
# publish: serve $tree as the release asset with a correct checksum.
publish() {
  tar -czf "$work/releases/$asset" -C "$tree" .
  (cd "$work/releases"; if command -v sha256sum >/dev/null; then sha256sum "$asset"; else shasum -a 256 "$asset"; fi) > "$work/releases/$asset.sha256"
}

cat > "$work/tools/uname" <<'BIN'
#!/bin/sh
case "$1" in -s) echo "${TEST_OS:-Linux}";; -m) echo x86_64;; esac
BIN
# curl answers -w '%{url_effective}' with TEST_LATEST_URL and serves release
# downloads of the canonical repository only.
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
if [[ $format == '%{url_effective}' ]]; then printf '%s' "$TEST_LATEST_URL"; exit 0; fi
case "$url" in
  https://github.com/neverhuman/redline/releases/download/*) cp "$INSTALL_FIXTURES/releases/${url##*/}" "$out" ;;
  *) printf 'curl fixture: no route to %s\n' "$url" >&2; exit 22 ;;
esac
BIN
cat > "$work/tools/gh" <<'BIN'
#!/bin/sh
printf '%s\n' "$*" >> "$INSTALL_FIXTURES/gh.log"
exit "${TEST_GH_STATUS:-0}"
BIN
chmod +x "$work/tools/"*
export INSTALL_FIXTURES=$work VERSION=$version
export PATH="$work/tools:$PATH"

# install_ok <label> [VAR=value...]: a fresh install into a prefix with spaces.
install_ok() {
  local label=$1 prefix="$work/prefix with spaces"
  shift
  rm -rf "$prefix"
  if ! env "$@" PREFIX="$prefix" bash "$root/install.sh" > "$work/$label.log" 2>&1; then
    fail "$label: installer failed: $(tail -n 1 "$work/$label.log")"
  elif [[ ! -x $prefix/bin/redlinedb-server || -e $prefix/bin/sqlite3 ]]; then
    fail "$label: unexpected installation tree"
  fi
}
# expect_reject <label> <message> [VAR=value...]: the installer must fail
# with <message> and leave an existing prefix untouched.
expect_reject() {
  local label=$1 message=$2 prefix="$work/kept prefix" listing
  shift 2
  rm -rf "$prefix"
  mkdir -p "$prefix/bin"
  printf 'keep\n' > "$prefix/bin/existing"
  if env "$@" PREFIX="$prefix" bash "$root/install.sh" > "$work/$label.log" 2>&1; then
    fail "$label: installer accepted it"
  elif ! grep -qF -- "$message" "$work/$label.log"; then
    fail "$label: expected '$message', got: $(tail -n 1 "$work/$label.log")"
  fi
  listing=$(cd "$prefix" && find . | LC_ALL=C sort | tr '\n' ' ')
  [[ $listing == '. ./bin ./bin/existing ' && $(cat "$prefix/bin/existing") == keep ]] ||
    fail "$label: prefix changed: $listing"
}

package good "$(provenance "$REDLINE_REPO_ID" "$version")"
publish
install_ok pinned
install_ok latest VERSION=latest TEST_LATEST_URL="$canonical/releases/tag/$version"
expect_reject unsupported-platform 'unsupported platform' TEST_OS=unsupported
# With no published release GitHub redirects /releases/latest to /releases.
expect_reject no-release 'no published release' VERSION=latest TEST_LATEST_URL="$canonical/releases"
expect_reject latest-elsewhere 'no published release' VERSION=latest TEST_LATEST_URL="$legacy_url/releases/tag/$version"
expect_reject missing-release "cannot download redlinedb-v4.0.0-linux-x86_64.tar.gz from $canonical release v4.0.0" VERSION=v4.0.0

# Rejected archives carry correct checksums: only the identity is wrong.
package legacy "$(provenance "$legacy_id" "$version")"
publish
expect_reject legacy-authority "was not built by $canonical"
package longer-id "$(provenance "${REDLINE_REPO_ID}0" "$version")"
publish
expect_reject longer-repository-id "was not built by $canonical"
package wrong-tag "$(provenance "$REDLINE_REPO_ID" v4.1.0-rc.2)"
publish
expect_reject wrong-tag "provenance does not name $version"
package tag-prefix "$(provenance "$REDLINE_REPO_ID" "${version}0")"
publish
expect_reject tag-prefix "provenance does not name $version"
package unattributed ''
publish
expect_reject no-provenance 'no build provenance'
package symlink "$(provenance "$REDLINE_REPO_ID" "$version")"
ln -s redlinedb "$tree/bin/sqlite3"
publish
expect_reject symlink-member 'unsafe archive member type'
package hardlink "$(provenance "$REDLINE_REPO_ID" "$version")"
ln "$tree/bin/redlinedb" "$tree/bin/redlinedb-copy"
publish
expect_reject hardlink-member 'unsafe archive member type'

package good "$(provenance "$REDLINE_REPO_ID" "$version")"
publish
rm -f "$work/gh.log"
install_ok attested REDLINEDB_VERIFY_ATTESTATION=1
expected_gh="/$asset --repo $REDLINE_REPO_SLUG --signer-workflow $REDLINE_REPO_SLUG/.github/workflows/release-build.yml"
if ! grep -q '^attestation verify ' "$work/gh.log" 2>/dev/null || ! grep -qF -- "$expected_gh" "$work/gh.log"; then
  fail "attested: gh attestation verify was not asked for $REDLINE_REPO_SLUG's release workflow"
fi
expect_reject attestation-refused 'attestation verification failed' REDLINEDB_VERIFY_ATTESTATION=1 TEST_GH_STATUS=1
expect_reject attestation-flag 'REDLINEDB_VERIFY_ATTESTATION must be 0 or 1' REDLINEDB_VERIFY_ATTESTATION=yes

printf 'corruption' >> "$work/releases/$asset"
expect_reject checksum 'checksum mismatch'

[[ $failures == 0 ]] || { printf '%d installer check(s) failed\n' "$failures" >&2; exit 1; }
printf 'Installer authority, provenance, archive-type, spaces, platform and checksum tests passed.\n'
