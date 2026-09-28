#!/usr/bin/env bash
# Noninteractive GitHub binary installer. No Rust, Node, sudo or sqlite3 alias.
set -euo pipefail
die() { printf 'redlinedb install: %s\n' "$*" >&2; exit 1; }
[[ $# == 0 ]] || { printf 'Usage: VERSION=v4.1.0 PREFIX=/installation/path bash install.sh\n' >&2; exit 64; }
# Release authority, repeated from ops/release/authority.env because a piped
# installer cannot read files. Archives must name this repository id.
repo_slug=neverhuman/redline
repo_id=1390165945
repo=https://github.com/$repo_slug
prefix=${PREFIX:-$HOME/.local}
attest=${REDLINEDB_VERIFY_ATTESTATION:-0}
[[ $attest == 0 || $attest == 1 ]] || die 'REDLINEDB_VERIFY_ATTESTATION must be 0 or 1'
case "$(uname -s)/$(uname -m)" in
  Linux/x86_64) platform=linux-x86_64 ;;
  Linux/aarch64|Linux/arm64) platform=linux-arm64 ;;
  Darwin/x86_64) platform=macos-x86_64 ;;
  Darwin/arm64) platform=macos-arm64 ;;
  *) die "unsupported platform: $(uname -s)/$(uname -m)" ;;
esac
for tool in curl tar mktemp grep; do command -v "$tool" >/dev/null || die "missing prerequisite: $tool"; done
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
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
for download in "$asset" "$asset.sha256"; do
  curl --proto '=https' --tlsv1.2 -fsSL "$repo/releases/download/$version/$download" -o "$tmp/$download" ||
    die "cannot download $download from $repo release $version"
done
read -r expected filename < "$tmp/$asset.sha256"
[[ $expected =~ ^[a-fA-F0-9]{64}$ && $filename == "$asset" ]] || die 'invalid checksum file'
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
  case "$member" in /*|../*|*/../*|*/..) die 'unsafe archive member' ;; esac
done < "$tmp/members"
# Release archives hold regular files and directories only.
tar -tvzf "$tmp/$asset" > "$tmp/listing"
while IFS= read -r entry; do
  case $entry in
    *' link to '*) die "unsafe archive member type: $entry" ;;
    [-d]*) ;;
    *) die "unsafe archive member type: $entry" ;;
  esac
done < "$tmp/listing"
mkdir "$tmp/package"
tar -xzf "$tmp/$asset" -C "$tmp/package"
[[ -x $tmp/package/bin/redlinedb && -x $tmp/package/bin/redlinedb-server ]] || die 'archive is missing CLI or server'
provenance=$tmp/package/share/redlinedb/build-provenance.json
[[ -f $provenance ]] || die "archive has no build provenance; it was not built by $repo"
grep -Eq "\"repository_id\":${repo_id}[,}]" "$provenance" || die "archive was not built by $repo (repository id $repo_id)"
grep -Fq "\"tag\":\"$version\"" "$provenance" || die "archive provenance does not name $version"
for dir in bin lib include share; do
  [[ ! -d $tmp/package/$dir ]] || { mkdir -p "$prefix/$dir"; cp -R "$tmp/package/$dir/." "$prefix/$dir/"; }
done
"$prefix/bin/redlinedb" --version
printf 'Installed %s in %s. Add %s/bin to PATH.\n' "$version" "$prefix" "$prefix"
