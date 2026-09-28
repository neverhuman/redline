#!/usr/bin/env bash
# Check the licence material that ships in release archives, and the collector
# (scripts/release/collect-licenses.sh) that assembles it.
#
#   scripts/test-package-licenses.sh [packages-dir]
#
# 1. The repository's LICENSE is the verbatim Apache-2.0 text, NOTICE exists,
#    and every package of every first-party Cargo workspace declares
#    Apache-2.0.
# 2. Collector fixture: a git-sourced crate that declares
#    license-file = "legal/custom.txt" has that file collected, and so is its
#    bundled vendor/c/LICENSE; a REUSE-style LICENSES/ directory is collected;
#    a dev-only dependency stays out of the inventory; a dependency with no
#    top-level licence text (only a nested one) fails the collection until it
#    is waived; a malformed waiver file and a first-party package that is not
#    Apache-2.0 fail it; npm production dependencies are collected under
#    licenses/npm/ and dev ones are not.
# 3. Every archive in packages-dir (default $OUTPUT_DIR, then target/packages):
#    share/redlinedb/LICENSE is the verbatim Apache-2.0 text, NOTICE exists, and
#    every non-workspace row of DEPENDENCIES.tsv has a non-empty licence
#    directory or a waiver in ops/release/license-waivers.toml.
#
# Needs bash, jq, git, cargo and npm; it builds nothing.
set -euo pipefail
# Point every git command at this script's fixtures, never at the caller's
# repository: a git hook (pre-push from a linked worktree) exports GIT_DIR,
# GIT_WORK_TREE and GIT_INDEX_FILE, and `git -C` does not override them.
while read -r variable; do unset "$variable"; done < <(git rev-parse --local-env-vars)
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=scripts/release/package-layout.sh
. "$root/scripts/release/package-layout.sh"
packages=${1:-${OUTPUT_DIR:-$root/target/packages}}
waivers=${LICENSE_WAIVERS:-$root/ops/release/license-waivers.toml}
collector=$root/scripts/release/collect-licenses.sh
apache_md5=3b83ef96387f14655fc854ddc3c6bd57
header=$'name\tversion\tlicense\trepository\tecosystem\tsource\tlicense_texts\twaiver'
for tool in jq git cargo npm; do
  command -v "$tool" >/dev/null || { printf 'missing prerequisite: %s\n' "$tool" >&2; exit 1; }
done
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }
md5_of() { if command -v md5sum >/dev/null; then md5sum "$1" | awk '{print $1}'; else md5 -q "$1"; fi; }

# waived <file> <ecosystem> <name> <version>: the file holds a [[waiver]] for
# exactly that package with a non-empty reason.
waived() {
  [[ -f $1 ]] || return 1
  awk -v eco="$2" -v name="$3" -v ver="$4" '
    function flush() { if (e == eco && p == name && v == ver && r != "") found = 1; e = p = v = r = "" }
    /^[[:space:]]*\[\[waiver\]\][[:space:]]*$/ { flush(); next }
    /^[[:space:]]*[a-z_]+[[:space:]]*=[[:space:]]*"/ {
      key = $0; sub(/^[[:space:]]*/, "", key); sub(/[[:space:]]*=.*$/, "", key)
      val = $0; sub(/^[^=]*=[[:space:]]*"/, "", val); sub(/"[[:space:]]*$/, "", val)
      if (key == "ecosystem") e = val; else if (key == "package") p = val
      else if (key == "version") v = val; else if (key == "reason") r = val
    }
    END { flush(); exit !found }' "$1"
}

# check_share <share-dir> <label> <waivers>: the licence contract of one
# installed share/redlinedb tree.
check_share() {
  local share=$1 label=$2 waiver_file=$3 rows=0 name version license ecosystem source texts waiver
  [[ -f $share/LICENSE && $(md5_of "$share/LICENSE") == "$apache_md5" ]] ||
    fail "$label: share/redlinedb/LICENSE is not the verbatim Apache-2.0 text (md5 $apache_md5)"
  [[ -s $share/NOTICE ]] || fail "$label: share/redlinedb/NOTICE is missing or empty"
  if [[ ! -f $share/DEPENDENCIES.tsv ]]; then fail "$label: DEPENDENCIES.tsv is missing"; return; fi
  if [[ $(head -n 1 "$share/DEPENDENCIES.tsv") != "$header" ]]; then
    fail "$label: DEPENDENCIES.tsv header is not: $header"
    return
  fi
  while IFS=$'\t' read -r name version license _ ecosystem source texts waiver; do
    rows=$((rows + 1))
    [[ -n $waiver ]] || { fail "$label: DEPENDENCIES.tsv row for $name has fewer than 8 columns"; continue; }
    if [[ $source == workspace ]]; then
      [[ $license == Apache-2.0 && $texts == LICENSE ]] ||
        fail "$label: first-party package $name $version is not covered by the Apache-2.0 LICENSE ($license, $texts)"
      continue
    fi
    if [[ $waiver == waived ]]; then
      waived "$waiver_file" "$ecosystem" "$name" "$version" ||
        fail "$label: $ecosystem package $name $version is marked waived but has no waiver in ${waiver_file#"$root"/}"
      continue
    fi
    case $texts in
      licenses/*) ;;
      *) fail "$label: $ecosystem package $name $version has no licence directory ($texts)"; continue ;;
    esac
    if [[ $texts == *..* || ! -d $share/$texts || -z $(find "$share/$texts" -type f -size +0 -print -quit) ]]; then
      fail "$label: $ecosystem package $name $version has an empty or missing $texts"
    fi
  done < <(tail -n +2 "$share/DEPENDENCIES.tsv")
  [[ $rows -gt 0 ]] || fail "$label: DEPENDENCIES.tsv lists no packages"
}

# 1. Repository licence and first-party manifests.
[[ $(md5_of "$root/LICENSE") == "$apache_md5" ]] || fail "LICENSE is not the verbatim Apache-2.0 text (md5 $apache_md5)"
[[ -s $root/NOTICE ]] || fail "NOTICE is missing or empty"
# Manifests are read directly (not through cargo) so a checkout nested inside
# another Cargo workspace cannot change the answer.
section_key() { # section_key <manifest> <section> <key-regex>: first matching line
  awk -v want="$2" -v key="$3" '/^[[:space:]]*\[/ { sec = $0; gsub(/[[:space:]]/, "", sec) }
    sec == want && $0 ~ ("^[[:space:]]*" key "[[:space:]]*=") { print; exit }' "$1"
}
while IFS= read -r manifest; do
  grep -q '^[[:space:]]*\[package\]' "$root/$manifest" || continue
  line=$(section_key "$root/$manifest" '[package]' 'license(\\.workspace)?')
  case $line in
    *'"Apache-2.0"'*) ;;
    *workspace*true*)
      dir=$(dirname "$manifest") inherited=
      while :; do
        if [[ -f $root/$dir/Cargo.toml ]] && grep -q '^[[:space:]]*\[workspace\.package\]' "$root/$dir/Cargo.toml"; then
          inherited=$(section_key "$root/$dir/Cargo.toml" '[workspace.package]' 'license')
          break
        fi
        [[ $dir != . ]] || break
        dir=$(dirname "$dir")
      done
      [[ $inherited == *'"Apache-2.0"'* ]] || fail "$manifest inherits a workspace licence that is not Apache-2.0 (${inherited:-none})"
      ;;
    *) fail "$manifest does not declare Apache-2.0 (${line:-no license key})" ;;
  esac
done < <(cd "$root" && git ls-files -- Cargo.toml '*/Cargo.toml' | grep -v '^subrepos/redline/')

# 2. Collector fixture. One local git repository holds the third-party crates.
deps=$work/deps app=$work/app
crate() { # crate <dir> <name> <licence-line> [file=content ...]
  local dir=$deps/$1 name=$2 licence=$3 entry
  shift 3
  mkdir -p "$dir/src"
  printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\n%s\n' "$name" "$licence" > "$dir/Cargo.toml"
  printf 'pub fn id() -> u8 { 0 }\n' > "$dir/src/lib.rs"
  for entry in "$@"; do mkdir -p "$(dirname "$dir/${entry%%=*}")"; printf '%s\n' "${entry#*=}" > "$dir/${entry%%=*}"; done
}
custom_text="fixture custom licence $$-$RANDOM"
crate custom custom-licensed 'license-file = "legal/custom.txt"' "legal/custom.txt=$custom_text" 'vendor/c/LICENSE=bundled C'

crate reuse reuse-licensed 'license = "MIT"' 'LICENSES/MIT.txt=MIT fixture text'
crate bare unlicensed 'license = "MIT"' 'third_party/COPYING=nested only'
crate devonly dev-only 'license = "MIT"'
git -C "$deps" init -q
git -C "$deps" add -A
git -C "$deps" -c user.name=fixture -c user.email=fixture@invalid -c commit.gpgsign=false commit -q -m fixture
mkdir -p "$app/src"
cat > "$app/Cargo.toml" <<TOML
[package]
name = "fixture-app"
version = "0.1.0"
edition = "2021"
license = "Apache-2.0"

[workspace]

[dependencies]
custom-licensed = { git = "file://$deps" }
reuse-licensed = { git = "file://$deps" }
unlicensed = { git = "file://$deps" }

[dev-dependencies]
dev-only = { git = "file://$deps" }
TOML
printf 'fn main() {}\n' > "$app/src/main.rs"
CARGO_HOME=$work/cargo-home cargo metadata --quiet --format-version 1 --manifest-path "$app/Cargo.toml" > "$work/fixture-metadata.json"
# npm fixture: one production and one dev dependency, installed by hand.
web=$work/web
mkdir -p "$web/node_modules/@scope/prod-dep" "$web/node_modules/dev-dep"
printf '{"name":"fixture-web","version":"0.0.0","private":true,"license":"Apache-2.0","dependencies":{"@scope/prod-dep":"1.0.0"},"devDependencies":{"dev-dep":"1.0.0"}}\n' > "$web/package.json"
printf '{"name":"@scope/prod-dep","version":"1.0.0","license":"MIT","repository":{"type":"git","url":"https://example.invalid/prod.git"}}\n' > "$web/node_modules/@scope/prod-dep/package.json"
printf 'prod dep licence\n' > "$web/node_modules/@scope/prod-dep/LICENSE.md"
printf '{"name":"dev-dep","version":"1.0.0","license":"MIT"}\n' > "$web/node_modules/dev-dep/package.json"
printf 'dev dep licence\n' > "$web/node_modules/dev-dep/LICENSE"
no_waivers=$work/no-waivers.toml
printf '# no waivers\n' > "$no_waivers"
fixture_waivers=$work/fixture-waivers.toml
cat > "$fixture_waivers" <<'TOML'
[[waiver]]
ecosystem = "cargo"
package = "unlicensed"
version = "0.1.0"
license = "MIT"
reason = "fixture: declares MIT and ships no text"
TOML
collect() { # collect <share> <waivers> [metadata]
  mkdir -p "$1"
  cp "$root/LICENSE" "$root/NOTICE" "$1/" 2>/dev/null || true
  bash "$collector" --metadata "${3:-$work/fixture-metadata.json}" --share "$1" --bin fixture-app \
    --npm-project "$web" --waivers "$2"
}
if [[ ! -f $collector ]]; then
  fail "collector ${collector#"$root"/} does not exist"
else
  if collect "$work/unwaived" "$no_waivers" > "$work/unwaived.log" 2>&1; then
    fail "collector accepted a dependency with no licence text"
  elif ! grep -q 'unlicensed 0.1.0' "$work/unwaived.log" || grep -qE 'custom-licensed|reuse-licensed' "$work/unwaived.log"; then
    fail "collector did not name exactly the unlicensed dependency: $(cat "$work/unwaived.log")"
  fi
  # Each malformed file is otherwise a valid waiver for unlicensed 0.1.0, so
  # only the waiver parser's own refusal can fail the run: a parser that
  # skipped the bad key or the missing one would accept the waiver and exit 0.
  cp "$fixture_waivers" "$work/bad-key.toml"
  printf 'colour = "red"\n' >> "$work/bad-key.toml"
  grep -v '^reason = ' "$fixture_waivers" > "$work/no-reason.toml"
  for bad in "bad-key:unknown waiver key colour" "no-reason:[[waiver]] is missing reason"; do
    name=${bad%%:*} want=${bad#*:}
    if collect "$work/$name" "$work/$name.toml" > "$work/$name.log" 2>&1; then
      fail "collector accepted the malformed waiver file $name.toml"
    elif ! grep -qF "$want" "$work/$name.log"; then
      fail "collector did not refuse $name.toml with '$want': $(cat "$work/$name.log")"
    fi
  done
  jq '(.packages[] | select(.name == "fixture-app") | .license) = "MIT"' "$work/fixture-metadata.json" > "$work/mit-app.json"
  if collect "$work/mit" "$fixture_waivers" "$work/mit-app.json" > "$work/mit.log" 2>&1; then
    fail "collector accepted a first-party package that is not Apache-2.0"
  fi
  share=$work/share
  if collect "$share" "$fixture_waivers" > "$work/collect.log" 2>&1; then
    custom=$(find "$share/licenses" -path '*/custom-licensed-0.1.0/legal/custom.txt' -type f)
    [[ -n $custom && $(cat "$custom") == "$custom_text" ]] || fail "license-file legal/custom.txt was not collected"
    [[ -f $share/licenses/custom-licensed-0.1.0/vendor/c/LICENSE ]] || fail "nested vendor/c/LICENSE was not collected"
    [[ -f $share/licenses/reuse-licensed-0.1.0/LICENSES/MIT.txt ]] || fail "LICENSES/ directory was not collected"
    [[ -f $share/licenses/unlicensed-0.1.0/third_party/COPYING ]] || fail "nested text of a waived package was not collected"
    [[ -f $share/licenses/npm/@scope/prod-dep-1.0.0/LICENSE.md ]] || fail "npm production licence was not collected"
    [[ ! -e $share/licenses/npm/dev-dep-1.0.0 ]] || fail "npm dev dependency licence was collected"
    ! grep -q '^dev-only' "$share/DEPENDENCIES.tsv" || fail "dev-only dependency is in DEPENDENCIES.tsv"
    ! grep -q '^dev-dep' "$share/DEPENDENCIES.tsv" || fail "npm dev dependency is in DEPENDENCIES.tsv"
    grep -q $'^fixture-app\t0.1.0\tApache-2.0\t-\tcargo\tworkspace\tLICENSE\t-$' "$share/DEPENDENCIES.tsv" ||
      fail "first-party row missing from DEPENDENCIES.tsv"
    grep -q $'^unlicensed\t0.1.0\tMIT\t-\tcargo\t.*\tlicenses/unlicensed-0.1.0\twaived$' "$share/DEPENDENCIES.tsv" ||
      fail "waived row missing from DEPENDENCIES.tsv"
    [[ $(jq '[.components[].name] | sort == ["custom-licensed","fixture-app","reuse-licensed","unlicensed"]' "$share/sbom.cdx.json") == true ]] ||
      fail "sbom.cdx.json does not list exactly the shipped graph"
    check_share "$share" fixture "$fixture_waivers"
  else
    fail "collector rejected the waived fixture: $(cat "$work/collect.log")"
  fi
fi

# 3. Built archives.
archives=("$packages"/*.tar.gz)
if [[ ! -f ${archives[0]} ]]; then
  fail "no release archives in $packages"
else
  for archive in "${archives[@]}"; do
    extract=$work/archive-$(basename "$archive" .tar.gz)
    mkdir -p "$extract"
    tar -xzf "$archive" -C "$extract"
    check_share "$extract/$(package_share "$(archive_package "$archive")")" "$(basename "$archive")" "$waivers"
  done
fi

if [[ $failures -gt 0 ]]; then
  printf '%d licence check(s) failed\n' "$failures" >&2
  exit 1
fi
printf 'Licence checks passed: repository, collector fixture, %d archive(s).\n' "${#archives[@]}"
