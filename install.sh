#!/usr/bin/env bash
# Noninteractive GitHub binary installer. No Rust, Node, sudo or sqlite3 alias.
#
# Layout: PREFIX/lib/redlinedb/versions/<tag>/{bin,lib,include,share} holds
# each installed version. PREFIX/lib/redlinedb/current and previous are links
# to versions. PREFIX/bin, PREFIX/lib and PREFIX/include hold stable links
# through current. The archive is downloaded and verified before anything under
# PREFIX is written, then staged and validated on the prefix's own filesystem
# and activated by renaming one link. A failed install leaves the version that
# was active before it active and whole.
#
# Environment:
#   VERSION                      release tag (default: the latest release)
#   PREFIX                       installation root (default: ~/.local)
#   REDLINEDB_SHA256             require this archive digest
#   REDLINEDB_VERIFY_ATTESTATION 1: also run gh attestation verify
#   REDLINEDB_ROLLBACK           1: make the previous version active again
#   REDLINEDB_MIGRATE_LEGACY     1: keep files of an earlier flat install in
#                                versions/legacy-<time>/ instead of refusing
#   REDLINEDB_LOCK_TIMEOUT       seconds to wait for another installer (300)
# scripts/install-from-source.sh sets REDLINEDB_LOCAL_PACKAGE (a package tree)
# and REDLINEDB_LOCAL_LABEL (its version name) to activate a local build.
set -euo pipefail
die() { printf 'redlinedb install: %s\n' "$*" >&2; exit 1; }
note() { printf 'redlinedb install: %s\n' "$*" >&2; }
[[ $# == 0 ]] || { printf 'Usage: VERSION=v5.0.0 PREFIX=/installation/path bash install.sh\n' >&2; exit 64; }
# Release authority, repeated from ops/release/authority.env because a piped
# installer cannot read files. Archives must name this repository id.
repo_slug=neverhuman/redline
repo_id=1390165945
repo=https://github.com/$repo_slug
prefix=${PREFIX:-$HOME/.local}
[[ -n $prefix ]] || die 'PREFIX is empty'
[[ $prefix == /* ]] || prefix=$PWD/$prefix
while [[ $prefix == */ && $prefix != / ]]; do prefix=${prefix%/}; done
base=$prefix/lib/redlinedb
for setting in "REDLINEDB_VERIFY_ATTESTATION=${REDLINEDB_VERIFY_ATTESTATION:-0}" \
  "REDLINEDB_ROLLBACK=${REDLINEDB_ROLLBACK:-0}" "REDLINEDB_MIGRATE_LEGACY=${REDLINEDB_MIGRATE_LEGACY:-0}"; do
  case ${setting#*=} in 0 | 1) ;; *) die "${setting%%=*} must be 0 or 1" ;; esac
done
attest=${REDLINEDB_VERIFY_ATTESTATION:-0}
rollback=${REDLINEDB_ROLLBACK:-0}
migrate=${REDLINEDB_MIGRATE_LEGACY:-0}
lock_timeout=${REDLINEDB_LOCK_TIMEOUT:-300}
[[ $lock_timeout =~ ^[0-9]+$ ]] || die 'REDLINEDB_LOCK_TIMEOUT must be a number of seconds'
local_package=${REDLINEDB_LOCAL_PACKAGE:-}
for tool in mktemp grep mkdir mv ln cp readlink cmp; do command -v "$tool" >/dev/null || die "missing prerequisite: $tool"; done

tmp='' stage='' pending='' locked=0
cleanup() {
  [[ -z $pending ]] || rm -f "$pending"
  [[ -z $stage ]] || rm -rf "$stage"
  [[ -z $tmp ]] || rm -rf "$tmp"
  [[ $locked == 0 ]] || rm -rf "$base/.lock"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
tmp=$(mktemp -d)

# The sqlite3.h shim includes redlinedb.h. In a directory the compiler searches
# by default it would shadow the system SQLite header, so it is not linked there.
system_include=0
case $prefix in /usr | /usr/local) system_include=1 ;; esac
if [[ -d $prefix/include ]]; then
  case $(cd "$prefix/include" 2>/dev/null && pwd -P || true) in /usr/include | /usr/local/include) system_include=1 ;; esac
fi

# entries <version dir>: the stable paths (bin/x, lib/x, include/x) a version
# provides, including the unversioned development link of its shared library.
entries() {
  local dir=$1 sub path name
  for sub in bin lib include; do
    for path in "$dir/$sub"/*; do
      [[ -e $path || -L $path ]] || continue
      name=${path##*/}
      [[ $sub/$name != include/sqlite3.h || $system_include == 0 ]] || continue
      printf '%s/%s\n' "$sub" "$name"
      case $sub/$name in
        lib/libredlinedb.so.[0-9]*) [[ -e $dir/lib/libredlinedb.so || -L $dir/lib/libredlinedb.so ]] || echo lib/libredlinedb.so ;;
        lib/libredlinedb.[0-9]*.dylib) [[ -e $dir/lib/libredlinedb.dylib || -L $dir/lib/libredlinedb.dylib ]] || echo lib/libredlinedb.dylib ;;
      esac
    done
  done
}
# foreign <entry>: true when PREFIX/<entry> exists and is not a link this
# installer made (one that goes through lib/redlinedb/current).
foreign() {
  local path=$prefix/$1
  if [[ -L $path ]]; then
    case $(readlink "$path") in */redlinedb/current/"$1" | redlinedb/current/"$1") return 1 ;; esac
    return 0
  fi
  [[ -e $path ]]
}
# active_copy <entry>: PREFIX/<entry> is the active version's own file (the
# same file, or a copy with the same bytes): what a migration that stopped
# part-way leaves at the paths it had not linked yet.
active_copy() {
  local path=$prefix/$1 active=$base/current/$1
  [[ -e $active ]] || return 1
  [[ $path -ef $active ]] && return 0
  [[ -f $path && ! -L $path && -f $active && ! -L $active ]] && cmp -s "$path" "$active"
}
# conflicts <version dir>: the foreign files that version's links would replace.
conflicts() {
  local entry found=''
  while IFS= read -r entry; do
    if foreign "$entry" && ! active_copy "$entry"; then found="$found $entry"; fi
  done < <(entries "$1")
  printf '%s' "$found"
}
refuse_conflicts() {
  [[ -n $1 && $migrate == 0 ]] || return 0
  die "these paths under $prefix were not made by this installer:$1. They look like an earlier flat installation. Rerun with REDLINEDB_MIGRATE_LEGACY=1 to keep them in $base/versions/legacy-<time>/ (the previous version) and replace them with links, or choose another PREFIX."
}
# validate <version dir>: the CLI and server exist and the CLI runs a query.
validated=''
validate() {
  local dir=$1 answer
  [[ -x $dir/bin/redlinedb && -x $dir/bin/redlinedb-server ]] || { note 'bin/redlinedb or bin/redlinedb-server is missing'; return 1; }
  validated=$(cd "$tmp" && "$dir/bin/redlinedb" --version 2>&1) || { note "redlinedb --version exited $?: $validated"; return 1; }
  answer=$(cd "$tmp" && "$dir/bin/redlinedb" -batch :memory: 'SELECT 1;' 2>&1) || { note "SELECT 1 exited $?: $answer"; return 1; }
  [[ $answer == 1 ]] || { note "SELECT 1 printed '$answer'"; return 1; }
}

# --- Choose the candidate without writing under PREFIX. ----------------------
if [[ $rollback == 1 ]]; then
  target=$(readlink "$base/previous" 2>/dev/null) || die "no previous version to roll back to in $base"
  [[ -d $base/$target ]] || die "the previous version $base/$target is missing"
  candidate=$base/$target
elif [[ -n $local_package ]]; then
  tag=${REDLINEDB_LOCAL_LABEL:-}
  [[ $tag =~ ^(source|local)-[A-Za-z0-9._-]+$ ]] || die "invalid REDLINEDB_LOCAL_LABEL: '$tag'"
  [[ -d $local_package ]] || die "REDLINEDB_LOCAL_PACKAGE is not a directory: $local_package"
  candidate=$(cd "$local_package" && pwd)
  target=versions/$tag
  record="source=local label=$tag"
else
  case "$(uname -s)/$(uname -m)" in
    Linux/x86_64) platform=linux-x86_64 ;;
    Linux/aarch64 | Linux/arm64) platform=linux-arm64 ;;
    Darwin/x86_64) platform=macos-x86_64 ;;
    Darwin/arm64) platform=macos-arm64 ;;
    *) die "unsupported platform: $(uname -s)/$(uname -m)" ;;
  esac
  # at_least <version> <major> <minor>
  at_least() {
    [[ $1 =~ ^([0-9]+)(\.([0-9]+))? ]] || return 1
    local major=${BASH_REMATCH[1]} minor=${BASH_REMATCH[3]:-0}
    ((major > $2 || (major == $2 && minor >= $3)))
  }
  case $platform in
    linux-*)
      libc=$(getconf GNU_LIBC_VERSION 2>/dev/null || true)
      libc=${libc#glibc }
      if [[ ! $libc =~ ^[0-9] ]]; then
        libc=$(ldd --version 2>/dev/null | head -n 1 || true)
        case $libc in *GLIBC* | *'GNU libc'*) libc=${libc##* } ;; *) libc='' ;; esac
      fi
      at_least "$libc" 2 35 ||
        die "the $platform package needs glibc 2.35 or newer; this system has ${libc:-no glibc}. Build from source instead (docs/install.md)."
      ;;
    macos-*)
      macos_version=$(sw_vers -productVersion 2>/dev/null || true)
      at_least "$macos_version" 15 0 ||
        die "the $platform package needs macOS 15 or newer; this system reports ${macos_version:-no version}. Build from source instead (docs/install.md)."
      ;;
  esac
  for tool in curl tar; do command -v "$tool" >/dev/null || die "missing prerequisite: $tool"; done
  [[ $attest == 0 ]] || command -v gh >/dev/null || die 'REDLINEDB_VERIFY_ATTESTATION=1 needs the GitHub CLI (gh)'
  version=${VERSION:-latest}
  if [[ $version == latest ]]; then
    url=$(curl --proto '=https' --tlsv1.2 -fsSL -o /dev/null -w '%{url_effective}' "$repo/releases/latest") ||
      die "cannot resolve $repo/releases/latest"
    # Without a published release GitHub redirects to the /releases page.
    case $url in
      "$repo/releases/tag/v"*) version=${url#"$repo/releases/tag/"} ;;
      *) die "no published release in $repo (latest resolves to ${url:-nothing}); set VERSION" ;;
    esac
  fi
  version=v${version#v}
  [[ $version =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$ ]] || die "invalid VERSION: $version"
  asset=redlinedb-$version-$platform.tar.gz
  for download in "$asset" "$asset.sha256"; do
    curl --proto '=https' --tlsv1.2 -fsSL "$repo/releases/download/$version/$download" -o "$tmp/$download" ||
      die "cannot download $download from $repo release $version"
  done
  read -r expected filename < "$tmp/$asset.sha256" || true
  [[ ${expected:-} =~ ^[a-fA-F0-9]{64}$ && ${filename:-} == "$asset" ]] || die 'invalid checksum file'
  if command -v sha256sum >/dev/null; then
    actual=$(sha256sum "$tmp/$asset"); actual=${actual%% *}
  elif command -v shasum >/dev/null; then
    actual=$(shasum -a 256 "$tmp/$asset"); actual=${actual%% *}
  else
    die 'sha256sum or shasum is required'
  fi
  [[ $actual == "$expected" ]] || die "checksum mismatch for $asset"
  [[ -z ${REDLINEDB_SHA256:-} || $actual == "$REDLINEDB_SHA256" ]] || die "pinned checksum mismatch for $asset"
  # The checksum only proves the download is intact. The attestation proves the
  # release workflow of the canonical repository built these bytes.
  if [[ $attest == 1 ]]; then
    gh attestation verify "$tmp/$asset" --repo "$repo_slug" --signer-workflow "$repo_slug/.github/workflows/release-build.yml" >&2 ||
      die "attestation verification failed for $asset"
  fi
  tar -tzf "$tmp/$asset" > "$tmp/members"
  while IFS= read -r member; do
    case "$member" in /* | ../* | */../* | */..) die 'unsafe archive member' ;; esac
  done < "$tmp/members"
  # Release archives hold regular files and directories only. GNU and BSD tar
  # list a hard link as "link to", BusyBox as "->" after a regular-file mode.
  tar -tvzf "$tmp/$asset" > "$tmp/listing"
  while IFS= read -r entry; do
    case $entry in
      *' link to '* | *' -> '*) die "unsafe archive member type: $entry" ;;
      [-d]*) ;;
      *) die "unsafe archive member type: $entry" ;;
    esac
  done < "$tmp/listing"
  mkdir "$tmp/package"
  tar -xzf "$tmp/$asset" -C "$tmp/package"
  candidate=$tmp/package
  [[ -x $candidate/bin/redlinedb && -x $candidate/bin/redlinedb-server ]] || die 'archive is missing CLI or server'
  provenance=share/redlinedb/build-provenance.json
  [[ -f $candidate/$provenance ]] || die "archive has no build provenance; it was not built by $repo"
  grep -Eq "\"repository_id\":${repo_id}[,}]" "$candidate/$provenance" || die "archive was not built by $repo (repository id $repo_id)"
  grep -Fq "\"tag\":\"$version\"" "$candidate/$provenance" || die "archive provenance does not name $version"
  target=versions/$version
  record="source=release asset=$asset sha256=$actual"
fi
[[ ! -e $candidate/bin/sqlite3 && ! -L $candidate/bin/sqlite3 ]] || die 'archive ships bin/sqlite3; this installer never installs a sqlite3 command'
[[ ! -e $base || -d $base ]] || die "$base exists and is not a directory"
refuse_conflicts "$(conflicts "$candidate")"

# --- Stage, validate and activate under the lock. ----------------------------
mkdir -p "$base/versions" || die "cannot create $base"
waited=0
until mkdir "$base/.lock" 2>/dev/null; do
  if [[ ! -d $base/.lock ]]; then
    # The holder may have released the lock between the two checks: try
    # again once before calling it an error.
    mkdir "$base/.lock" 2>/dev/null && break
    [[ -d $base/.lock ]] || die "cannot create $base/.lock"
  fi
  if [[ $waited == 0 ]]; then
    note "waiting for another installer ($(cat "$base/.lock/owner" 2>/dev/null || echo 'owner not recorded yet')) to release $base/.lock"
  fi
  [[ $waited -lt $lock_timeout ]] ||
    die "timed out after ${lock_timeout}s waiting for $base/.lock. If no installer is running (see $base/.lock/owner), remove that directory and rerun."
  sleep 1
  waited=$((waited + 1))
done
locked=1
printf 'pid %s on %s since %s\n' "$$" "$(uname -n)" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$base/.lock/owner" || true
base_physical=$(cd "$base" && pwd -P)
# Under the lock no other installer is running, so a staging directory left
# by one that was killed is garbage.
for leftover in "$base"/.stage.*; do [[ ! -d $leftover ]] || rm -rf "$leftover"; done
stage=$(mktemp -d "$base/.stage.XXXXXX")

# Replacing a link must be one rename(2): GNU mv needs -T and BSD mv needs -h,
# or mv would move the new link into the directory the old one points to.
mkdir "$stage/probe" "$stage/probe/dir"
ln -s dir "$stage/probe/current" && ln -s dir "$stage/probe/new" || die "cannot create links in $base"
if probe=$(mv -T "$stage/probe/new" "$stage/probe/current" 2>&1) && [[ -L $stage/probe/current && ! -L $stage/probe/new && ! -L $stage/probe/dir/new ]]; then
  replace=-T
else
  rm -f "$stage/probe/dir/new" "$stage/probe/new"
  ln -s dir "$stage/probe/new" || die "cannot create links in $base"
  if probe="$probe $(mv -h "$stage/probe/new" "$stage/probe/current" 2>&1)" && [[ -L $stage/probe/current && ! -L $stage/probe/new && ! -L $stage/probe/dir/new ]]; then
    replace=-h
  else
    die "mv cannot replace a link in one step here (needs GNU mv -T or BSD mv -h): $probe"
  fi
fi
rm -rf "$stage/probe"
# put_link <link target> <path>: create or replace <path> atomically.
put_link() {
  pending=$2.redlinedb-new
  rm -f "$pending"
  ln -s "$1" "$pending" || return 1
  mv "$replace" "$pending" "$2" || return 1
  pending=''
}
stable_target() {
  case $1 in
    lib/*) printf 'redlinedb/current/%s' "$1" ;;
    *) if [[ -L $prefix/${1%%/*} ]]; then printf '%s/current/%s' "$base_physical" "$1"; else printf '../lib/redlinedb/current/%s' "$1"; fi ;;
  esac
}
previous_active=$(readlink "$base/current" 2>/dev/null || true)

if [[ $rollback == 1 ]]; then
  target=$(readlink "$base/previous" 2>/dev/null) || die "no previous version to roll back to in $base"
  candidate=$base/$target
  validate "$candidate" || die "the previous version $target failed validation; nothing changed"
else
  cp -pR "$candidate/." "$stage/" || die "cannot copy the candidate into $stage; nothing changed"
  # The development link lives in the version tree so it switches with current.
  for library in "$stage/lib/"libredlinedb.so.[0-9]* "$stage/lib/"libredlinedb.[0-9]*.dylib; do
    [[ -f $library && ! -L $library ]] || continue
    case $library in *.dylib) dev=libredlinedb.dylib ;; *) dev=libredlinedb.so ;; esac
    [[ -e $stage/lib/$dev || -L $stage/lib/$dev ]] || ln -s "${library##*/}" "$stage/lib/$dev" ||
      die "cannot create the development link $dev; nothing changed"
  done
  validate "$stage" || die "candidate failed validation; the active installation is unchanged"
  if [[ -z $local_package ]]; then
    grep -Eq "\"repository_id\":${repo_id}[,}]" "$stage/$provenance" && grep -Fq "\"tag\":\"$version\"" "$stage/$provenance" ||
      die 'staged provenance differs from the verified archive; nothing changed'
  fi
  printf '%s\n' "$record" > "$stage/.redlinedb-install" || die "cannot write $stage/.redlinedb-install"
  if [[ -e $base/$target || -L $base/$target ]]; then
    # Release archives are immutable, so the same tag is the same bytes.
    [[ $(cat "$base/$target/.redlinedb-install" 2>/dev/null) == "$record" ]] && validate "$base/$target" ||
      die "$base/$target already exists with other contents; remove it (roll back first if it is active) and rerun"
    rm -rf "$stage"
  else
    mv "$stage" "$base/$target" || die "cannot move the staged version into $base/$target; nothing changed"
  fi
  stage=''

  candidate=$base/$target
fi

# Files of an earlier flat installation are kept, with their paths, in a
# legacy version, which becomes the active one until the switch below. They
# are hard-linked (or copied) there and the originals stay in place; each is
# then replaced by its stable link in one rename, and that link reaches the
# same bytes through current. So at every step, and after a failure or an
# interrupt at any step, every legacy path still answers with the legacy file.
found=$(conflicts "$candidate")
migrated=' '
if [[ -n $found ]]; then
  refuse_conflicts "$found"
  legacy=versions/legacy-$(date -u +%Y%m%dT%H%M%SZ)
  [[ ! -e $base/$legacy ]] || legacy=$legacy-$$
  for entry in $found; do
    if ! { mkdir -p "$base/$legacy/${entry%/*}" &&
      { ln -P "$prefix/$entry" "$base/$legacy/$entry" 2>/dev/null || cp -pP "$prefix/$entry" "$base/$legacy/$entry"; }; }; then
      rm -rf "${base:?}/$legacy"
      die "cannot keep $prefix/$entry in $base/$legacy; nothing changed"
    fi
  done
  if [[ -z $previous_active ]]; then
    if ! put_link "$legacy" "$base/current"; then
      rm -rf "${base:?}/$legacy"
      die "cannot activate $base/$legacy; nothing changed"
    fi
    previous_active=$legacy
  fi
  note "kept$found in $base/$legacy"
  migrated="$found "
fi
while IFS= read -r entry; do
  want=$(stable_target "$entry")
  if [[ -L $prefix/$entry && $(readlink "$prefix/$entry") == "$want" ]]; then continue; fi
  if [[ $migrated != *" $entry "* ]] && foreign "$entry" && ! active_copy "$entry"; then
    die "$prefix/$entry appeared during the install; nothing was activated"
  fi
  mkdir -p "$prefix/${entry%/*}" && put_link "$want" "$prefix/$entry" || die "cannot link $prefix/$entry; nothing was activated"
done < <(entries "$candidate")
# Record the rollback target, then switch. If the switch fails, put the
# rollback target back, so a failed install changes neither link.
earlier_previous=$(readlink "$base/previous" 2>/dev/null || true)
if [[ -n $previous_active && $previous_active != "$target" ]]; then
  put_link "$previous_active" "$base/previous" || die "cannot record the previous version; nothing was activated"
fi
# The switch: one rename makes every stable link reach the new version.
if ! put_link "$target" "$base/current"; then
  [[ -z $pending ]] || rm -f "$pending"
  pending=''
  if [[ -n $earlier_previous ]]; then
    put_link "$earlier_previous" "$base/previous" ||
      note "could not restore $base/previous to $earlier_previous; it names ${previous_active:-nothing}"
  else
    rm -f "$base/previous"
  fi
  die "cannot activate $target; nothing was activated"
fi

# Links of files the active version does not have dangle now; remove them.
for sub in bin lib include; do
  for path in "$prefix/$sub"/*; do
    [[ -L $path && ! -e $path ]] || continue
    case $(readlink "$path") in */redlinedb/current/"$sub/${path##*/}" | redlinedb/current/"$sub/${path##*/}") rm -f "$path" ;; esac
  done
done
[[ $prefix/bin/redlinedb -ef $candidate/bin/redlinedb ]] || die "activated $target, but $prefix/bin/redlinedb does not reach it"
[[ $system_include == 0 ]] ||
  note "sqlite3.h is not linked into $prefix/include, where it would shadow the system SQLite header; it is in $base/current/include"
printf '%s\n' "$validated"
printf 'Active: %s (%s)' "${target#versions/}" "$base/current"
[[ $previous_active == "$target" || -z $previous_active ]] || printf '; previous: %s' "${previous_active#versions/}"
printf '\n'
# shellcheck disable=SC2016 # the hint prints $PATH literally
case ":$PATH:" in
  *":$prefix/bin:"*) ;;
  *) printf 'Add %s/bin to PATH, for example: export PATH="%s/bin:$PATH"\n' "$prefix" "$prefix" ;;
esac
