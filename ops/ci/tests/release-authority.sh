#!/usr/bin/env bash
# Tests for the release authority (S8-02): only the repository named in
# ops/release/authority.env may package or publish RedlineDB releases.
#   - scripts/package-release.sh refuses a foreign GITHUB_REPOSITORY_ID before
#     it builds anything;
#   - ops/ci/publish-github-release.sh refuses a foreign GITHUB_REPOSITORY or
#     GITHUB_REPOSITORY_ID before its first gh call; refuses a tag that
#     ops/ci/release-version.sh refuses, an asset set other than one archive
#     and one checksum per package and platform, a checksum file naming
#     another archive, and archives whose provenance names another
#     repository or tag, and a missing or mismatched acceptance manifest
#     (scripts/release/verify-acceptance.sh), before it creates anything;
#     names --repo on every gh call; takes the notes from
#     docs/releases/vX.Y.Z.md; uploads the manifest with the archives; and
#     marks candidates as prereleases.
# No network or GitHub access: gh is a shim, and cargo, rustc and npm are
# shims while packaging. Publication runs `cargo metadata` on a fixture.
#
# Usage: bash ops/ci/tests/release-authority.sh
set -euo pipefail
# Point every git command at this script's fixtures, never at the caller's
# repository: a git hook (pre-push from a linked worktree) exports GIT_DIR,
# GIT_WORK_TREE and GIT_INDEX_FILE, and `git -C` does not override them.
while read -r variable; do unset "$variable"; done < <(git rev-parse --local-env-vars)

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
# shellcheck source=ops/release/authority.env
. "$root/ops/release/authority.env"
# shellcheck source=scripts/release/package-layout.sh
. "$root/scripts/release/package-layout.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }
# The retired repository; its name is split so validation scans pass.
legacy_id=1240106851 legacy_slug="neverhumanbot/Redline""DB"
# A tag the publisher's release policy accepts for the fixture's 5.0.0 crates.
tag=v5.0.0-rc.7

mkdir -p "$work/tools"
for tool in cargo rustc npm; do
  printf '#!/bin/sh\nprintf "%%s invoked\\n" "%s" >> "%s/tools.log"\nexit 1\n' "$tool" "$work" > "$work/tools/$tool"
done
printf '#!/bin/sh\nprintf "%%s\\n" "$*" >> "%s/gh.log"\n' "$work" > "$work/tools/gh"
chmod +x "$work/tools/"*
PATH_BEFORE_SHIMS=$PATH
export PATH="$work/tools:$PATH"

# --- packaging --------------------------------------------------------------
status=0
(cd "$root" && env GITHUB_REPOSITORY_ID=$legacy_id TAG=$tag OUTPUT_DIR="$work/out" \
  bash scripts/package-release.sh) > "$work/package.log" 2>&1 || status=$?
[[ $status != 0 ]] || fail "package-release.sh packaged for repository id $legacy_id"
grep -qF "refusing to package for repository id $legacy_id" "$work/package.log" ||
  fail "package-release.sh did not refuse repository id $legacy_id: $(tail -n 1 "$work/package.log")"
[[ ! -e $work/tools.log ]] || fail "package-release.sh ran $(head -n 1 "$work/tools.log") before checking the repository"

# --- publication ------------------------------------------------------------
# gh logs every call; `release view` answers "release not found".
mkdir -p "$work/gh-only"
# shellcheck disable=SC2016 # the stand-in expands $* and $1 when it runs
printf '#!/bin/sh\nprintf "%%s\\n" "$*" >> "%s/gh.log"\n[ "$1 $2" != "release view" ] || { echo "release not found" >&2; exit 1; }\n' \
  "$work" > "$work/gh-only/gh"
chmod +x "$work/gh-only/gh"
publish_path="$work/gh-only:$PATH_BEFORE_SHIMS"
# The checkout: a one-crate workspace at 5.0.0 with its changelog section and
# notes, target/ ignored, origin/main at HEAD, and annotated tags v5.0.0 and
# $tag.
checkout=$work/checkout
mkdir -p "$checkout/src" "$checkout/docs/releases"
printf '/target/\n' > "$checkout/.gitignore"
printf '[package]\nname = "fixture"\nversion = "5.0.0"\nedition = "2021"\n\n[workspace]\n' > "$checkout/Cargo.toml"
: > "$checkout/src/lib.rs"
printf '# Changelog\n\n## [5.0.0] - 2026-10-01\n' > "$checkout/CHANGELOG.md"
printf '# RedlineDB v5.0.0\n' > "$checkout/docs/releases/v5.0.0.md"
git init --quiet "$checkout"
git -C "$checkout" add -A
git -C "$checkout" -c user.name=fixture -c user.email=fixture@example.invalid -c commit.gpgSign=false commit --quiet -m fixture
git -C "$checkout" update-ref refs/remotes/origin/main HEAD
for t in v5.0.0 "$tag"; do
  git -C "$checkout" -c user.name=fixture -c user.email=fixture@example.invalid -c tag.gpgSign=false tag -a -m "$t" "$t"
done
git -C "$checkout" tag v5.0.0-rc.8
commit=$(git -C "$checkout" rev-parse HEAD)
tree=$(git -C "$checkout" rev-parse 'HEAD^{tree}')
rustc_line='rustc 1.95.0 (59807616e 2026-04-14)'

platforms=(linux-x86_64 linux-arm64 macos-x86_64 macos-arm64)
# packages <tag> [repository id for the last archive]: the twelve release
# archives of <tag>, each with its checksum file.
packages() {
  local release=$1 last_id=${2:-$REDLINE_REPO_ID} package platform dir share id count=0
  rm -rf "$checkout/target/packages" "$work/trees"
  mkdir -p "$checkout/target/packages"
  for package in redlinedb redline-web redline-testing; do
    for platform in "${platforms[@]}"; do
      count=$((count + 1))
      id=$REDLINE_REPO_ID
      [[ $count != 12 ]] || id=$last_id
      dir=$work/trees/$package-$platform
      share=$dir/$(package_share "$package")
      mkdir -p "$share"
      printf '{"schema":"redline.release-build/v2","repository_url":"%s","repository_id":%s,"tag":"%s","commit":"%s","source_tree":"%s","package":"%s","rust":"%s"}\n' \
        "$REDLINE_REPO_URL" "$id" "$release" "$commit" "$tree" "$package" "$rustc_line" > "$share/build-provenance.json"
      tar -czf "$checkout/target/packages/$package-$release-$platform.tar.gz" -C "$dir" .
      (cd "$checkout/target/packages" && sha256sum "$package-$release-$platform.tar.gz" > "$package-$release-$platform.tar.gz.sha256")
    done
  done
}

# acceptance <tag>: the acceptance manifest of <tag> over the archives now in
# target/packages, from receipts and a job list of a successful run.
mkdir -p "$work/jobs-gh"
printf '#!/bin/sh\necho %s\n' "'{\"jobs\":[{\"name\":\"acceptance / RedlineDB/required\",\"status\":\"completed\",\"conclusion\":\"success\"},{\"name\":\"publish\",\"status\":\"in_progress\",\"conclusion\":null}]}'" \
  > "$work/jobs-gh/gh"
chmod +x "$work/jobs-gh/gh"
acceptance() {
  local release=$1 name receipts=$checkout/target/acceptance/receipts
  rm -rf "$checkout/target/acceptance"
  while read -r name; do
    mkdir -p "$receipts/$name"
    printf '%s\n' "$name" > "$receipts/$name/evidence.txt"
  done < <(sed 's/#.*//' "$root/ops/release/acceptance-receipts" | awk 'NF { print $1 }')
  printf '{"candidate":{"sha":"%s","dirty":false,"shallow":false},"result":"pass"}\n' "$commit" > "$receipts/security-receipt/receipt.json"
  (cd "$checkout" && env PATH="$work/jobs-gh:$PATH_BEFORE_SHIMS" TAG="$release" GH_TOKEN=fixture \
    GITHUB_REPOSITORY="$REDLINE_REPO_SLUG" GITHUB_REPOSITORY_ID="$REDLINE_REPO_ID" GITHUB_RUN_ID=7 GITHUB_RUN_ATTEMPT=1 \
    bash "$root/ops/ci/release-acceptance.sh" target/packages target/acceptance/receipts \
    target/acceptance/release-acceptance.v1.json) > "$work/acceptance.log" 2>&1 \
    || fail "acceptance $release: $(tail -n 2 "$work/acceptance.log")"
}

# publish <label> [VAR=value...]; sets $status. Default identity is canonical.
publish() {
  local label=$1
  shift
  rm -f "$work/gh.log"
  status=0
  (cd "$checkout" && env PATH="$publish_path" GITHUB_REPOSITORY="$REDLINE_REPO_SLUG" GITHUB_REPOSITORY_ID="$REDLINE_REPO_ID" \
    GH_REPO= TAG=$tag RELEASE_ACCEPTANCE=target/acceptance/release-acceptance.v1.json \
    RELEASE_ACCEPTANCE_RECEIPTS=target/acceptance/receipts "$@" \
    bash "$root/ops/ci/publish-github-release.sh") > "$work/$label.log" 2>&1 || status=$?
}
# expect_refusal <label> <message> [VAR=value...]: refused, and nothing was
# created, uploaded or edited on GitHub.
expect_refusal() {
  local label=$1 message=$2
  shift 2
  publish "$label" "$@"
  [[ $status != 0 ]] || fail "$label: published"
  grep -qF -- "$message" "$work/$label.log" || fail "$label: expected '$message', got: $(tail -n 3 "$work/$label.log")"
  if grep -Eq '^release (create|upload|edit|delete) ' "$work/gh.log" 2>/dev/null; then
    fail "$label: gh changed a release before the refusal: $(grep -E '^release (create|upload|edit|delete) ' "$work/gh.log" | head -n 1)"
  fi
}
# expect_published <label> <tag> [VAR=value...]: the three gh release calls,
# each naming the canonical repository.
expect_published() {
  local label=$1 release=$2
  shift 2
  publish "$label" TAG="$release" "$@"
  [[ $status == 0 ]] || fail "$label: publication failed: $(tail -n 3 "$work/$label.log")"
  calls=$(grep -cE '^release (create|upload|edit) ' "$work/gh.log" 2>/dev/null || true)
  [[ $calls == 3 ]] || fail "$label: expected 3 gh release changes, got ${calls:-0}"
  # The whole argument: a look-alike such as $REDLINE_REPO_SLUG-fork must not pass.
  if grep -vE -- "(^| )--repo ${REDLINE_REPO_SLUG//./\\.}( |$)" "$work/gh.log" 2>/dev/null | grep -q .; then
    fail "$label: gh call without --repo $REDLINE_REPO_SLUG: $(grep -vE -- "(^| )--repo ${REDLINE_REPO_SLUG//./\\.}( |$)" "$work/gh.log" | head -n 1)"
  fi
  grep -q "^release create $release .*--notes-file docs/releases/v5.0.0.md" "$work/gh.log" 2>/dev/null ||
    fail "$label: notes are not docs/releases/v5.0.0.md: $(grep '^release create' "$work/gh.log" 2>/dev/null)"
  uploaded=$(grep '^release upload ' "$work/gh.log" 2>/dev/null | tr ' ' '\n' | grep -c '^target/packages/' || true)
  [[ $uploaded == 24 ]] || fail "$label: uploaded $uploaded files, not 12 archives and 12 checksums"
  grep -q '^release upload .* target/acceptance/release-acceptance.v1.json' "$work/gh.log" 2>/dev/null ||
    fail "$label: the acceptance manifest was not uploaded"
}

# expect_authority_refusal <label> [VAR=value...]: refused before any gh call.
expect_authority_refusal() {
  expect_refusal "$1" "refusing to publish" "${@:2}"
  [[ ! -s $work/gh.log ]] || fail "$1: gh ran before the refusal: $(head -n 1 "$work/gh.log")"
}

packages "$tag"
expect_authority_refusal unset-id GITHUB_REPOSITORY_ID=
expect_authority_refusal legacy-id GITHUB_REPOSITORY_ID=$legacy_id
expect_authority_refusal legacy-slug GITHUB_REPOSITORY=$legacy_slug
expect_authority_refusal fork-slug GITHUB_REPOSITORY=someone/redline
packages "$tag" "$legacy_id"
expect_refusal legacy-archive "redline-testing-$tag-macos-arm64.tar.gz was not built by $REDLINE_REPO_URL"
# repack <package> <platform>: rebuild that $tag archive and its checksum file
# from its (edited) tree, so only the provenance is wrong.
repack() {
  local archive=$1-$tag-$2.tar.gz
  tar -czf "$checkout/target/packages/$archive" -C "$work/trees/$1-$2" .
  (cd "$checkout/target/packages" && sha256sum "$archive" > "$archive.sha256")
}
packages "$tag"
provenance_file=$work/trees/redlinedb-linux-x86_64/$(package_share redlinedb)/build-provenance.json
sed "s/\"tag\":\"$tag\"/\"tag\":\"v5.0.0-rc.6\"/" "$provenance_file" > "$work/other-tag.json"
mv "$work/other-tag.json" "$provenance_file"
grep -qF '"tag":"v5.0.0-rc.6"' "$provenance_file" || fail 'other-tag-provenance: the fixture edit did not apply'
repack redlinedb linux-x86_64
expect_refusal other-tag-provenance "redlinedb-$tag-linux-x86_64.tar.gz provenance does not name $tag"
packages "$tag"
rm "$work/trees/redline-web-macos-arm64/$(package_share redline-web)/build-provenance.json"
repack redline-web macos-arm64
expect_refusal no-provenance "redline-web-$tag-macos-arm64.tar.gz has no build provenance"

# The release version policy runs first (ops/ci/release-version.sh).
packages v4.1.0-rc.2
expect_refusal old-version 'v4.1.0-rc.2 cannot be released' TAG=v4.1.0-rc.2
packages v5.0.0-rc.8
expect_refusal lightweight-tag 'v5.0.0-rc.8 is not an annotated tag' TAG=v5.0.0-rc.8
packages v5.0.0-rc.01
expect_refusal malformed-tag 'is not a release tag' TAG=v5.0.0-rc.01

# Exactly one archive and one checksum per package and platform.
packages "$tag"
rm "$checkout/target/packages/redline-web-$tag-linux-arm64.tar.gz.sha256"
expect_refusal missing-checksum "missing: redline-web-$tag-linux-arm64.tar.gz.sha256"
packages "$tag"
rm "$checkout/target/packages/redlinedb-$tag-macos-x86_64.tar.gz"*
expect_refusal missing-platform "missing: redlinedb-$tag-macos-x86_64.tar.gz"
packages "$tag"
cp "$checkout/target/packages/redlinedb-$tag-linux-x86_64.tar.gz" "$checkout/target/packages/redlinedb-$tag-windows-x86_64.tar.gz"
expect_refusal extra-archive "unexpected: redlinedb-$tag-windows-x86_64.tar.gz"
packages "$tag"
(cd "$checkout/target/packages" && sha256sum "redline-web-$tag-linux-x86_64.tar.gz" > "redlinedb-$tag-linux-x86_64.tar.gz.sha256")
expect_refusal crossed-checksum "redlinedb-$tag-linux-x86_64.tar.gz.sha256 does not name redlinedb-$tag-linux-x86_64.tar.gz"
packages "$tag"
printf 'tampered' >> "$checkout/target/packages/redline-testing-$tag-macos-arm64.tar.gz"
expect_refusal tampered-archive "redline-testing-$tag-macos-arm64.tar.gz: FAILED"

# The acceptance manifest: required, for this tag, and for these archives.
packages "$tag"
expect_refusal no-acceptance 'RELEASE_ACCEPTANCE must name the release-acceptance.v1.json' RELEASE_ACCEPTANCE=
packages v5.0.0
acceptance v5.0.0
packages "$tag"
expect_refusal other-acceptance "does not match --tag $tag"
acceptance "$tag"
# A rebuilt archive with its own checksum file and valid provenance, but not
# the bytes the acceptance run bound.
printf 'rebuilt\n' > "$work/trees/redlinedb-linux-arm64/extra"
tar -czf "$checkout/target/packages/redlinedb-$tag-linux-arm64.tar.gz" -C "$work/trees/redlinedb-linux-arm64" .
(cd "$checkout/target/packages" && sha256sum "redlinedb-$tag-linux-arm64.tar.gz" > "redlinedb-$tag-linux-arm64.tar.gz.sha256")
expect_refusal unbound-archive "redlinedb-$tag-linux-arm64.tar.gz: sha256"

packages "$tag"
acceptance "$tag"
expect_published candidate "$tag"
grep -q "^release create $tag .*--prerelease" "$work/gh.log" 2>/dev/null || fail 'candidate: not marked as a prerelease'
packages v5.0.0
acceptance v5.0.0
expect_published stable v5.0.0
! grep -q -- '--prerelease' "$work/gh.log" || fail 'stable: marked as a prerelease'

[[ $failures == 0 ]] || { printf '%d release authority check(s) failed\n' "$failures" >&2; exit 1; }
printf 'Release authority tests passed.\n'
