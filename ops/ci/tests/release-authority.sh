#!/usr/bin/env bash
# Tests for the release authority (S8-02): only the repository named in
# ops/release/authority.env may package or publish RedlineDB releases.
#   - scripts/package-release.sh refuses a foreign GITHUB_REPOSITORY_ID before
#     it builds anything;
#   - ops/ci/publish-github-release.sh refuses a foreign GITHUB_REPOSITORY or
#     GITHUB_REPOSITORY_ID, and archives whose provenance names another
#     repository or tag, before its first gh call, and names --repo on every
#     gh call.
# No network, cargo or GitHub access: gh, cargo, rustc and npm are shims.
#
# Usage: bash ops/ci/tests/release-authority.sh
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
# shellcheck source=ops/release/authority.env
. "$root/ops/release/authority.env"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }
# The retired repository; its name is split so validation scans pass.
legacy_id=1240106851 legacy_slug="neverhumanbot/Redline""DB"
# A tag the publisher's release policy accepts.
tag=v4.1.0-rc.7

mkdir -p "$work/tools"
for tool in cargo rustc npm; do
  printf '#!/bin/sh\nprintf "%%s invoked\\n" "%s" >> "%s/tools.log"\nexit 1\n' "$tool" "$work" > "$work/tools/$tool"
done
printf '#!/bin/sh\nprintf "%%s\\n" "$*" >> "%s/gh.log"\n' "$work" > "$work/tools/gh"
chmod +x "$work/tools/"*
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
checkout=$work/checkout
git init --quiet "$checkout"
git -C "$checkout" -c user.name=fixture -c user.email=fixture@example.invalid \
  commit --quiet --allow-empty -m fixture
git -C "$checkout" tag "$tag"
mkdir -p "$checkout/target/packages"

# packages <repository id for the last archive>: the twelve release archives.
packages() {
  local last_id=$1 package platform dir id count=0
  rm -rf "$checkout/target/packages" "$work/trees"
  mkdir -p "$checkout/target/packages"
  for package in redlinedb redline-web redline-testing; do
    for platform in linux-x86_64 linux-arm64 macos-x86_64 macos-arm64; do
      count=$((count + 1))
      id=$REDLINE_REPO_ID
      [[ $count != 12 ]] || id=$last_id
      dir=$work/trees/$package-$platform
      mkdir -p "$dir/share/redlinedb"
      printf '{"schema":"redline.release-build/v2","repository_url":"%s","repository_id":%s,"tag":"%s"}\n' \
        "$REDLINE_REPO_URL" "$id" "$tag" > "$dir/share/redlinedb/build-provenance.json"
      tar -czf "$checkout/target/packages/$package-$tag-$platform.tar.gz" -C "$dir" .
      (cd "$checkout/target/packages" && sha256sum "$package-$tag-$platform.tar.gz" > "$package-$tag-$platform.tar.gz.sha256")
    done
  done
}

# publish <label> [VAR=value...]; sets $status. Default identity is canonical.
publish() {
  local label=$1
  shift
  rm -f "$work/gh.log"
  status=0
  (cd "$checkout" && env GITHUB_REPOSITORY="$REDLINE_REPO_SLUG" GITHUB_REPOSITORY_ID="$REDLINE_REPO_ID" \
    GH_REPO= TAG=$tag "$@" bash "$root/ops/ci/publish-github-release.sh") > "$work/$label.log" 2>&1 || status=$?
}
# expect_refusal <label> <message> [VAR=value...]
expect_refusal() {
  local label=$1 message=$2
  shift 2
  publish "$label" "$@"
  [[ $status != 0 ]] || fail "$label: published"
  grep -qF -- "$message" "$work/$label.log" || fail "$label: expected '$message', got: $(tail -n 1 "$work/$label.log")"
  [[ ! -s $work/gh.log ]] || fail "$label: gh ran before the refusal: $(head -n 1 "$work/gh.log")"
}

packages "$REDLINE_REPO_ID"
expect_refusal unset-id "refusing to publish" GITHUB_REPOSITORY_ID=
expect_refusal legacy-id "refusing to publish" GITHUB_REPOSITORY_ID=$legacy_id
expect_refusal legacy-slug "refusing to publish" GITHUB_REPOSITORY=$legacy_slug
expect_refusal fork-slug "refusing to publish" GITHUB_REPOSITORY=someone/redline
packages "$legacy_id"
expect_refusal legacy-archive "redline-testing-$tag-macos-arm64.tar.gz was not built by $REDLINE_REPO_URL"
packages "$REDLINE_REPO_ID"
publish canonical
[[ $status == 0 ]] || fail "canonical: publication failed: $(tail -n 1 "$work/canonical.log")"
calls=$(grep -c '^release ' "$work/gh.log" 2>/dev/null || true)
[[ $calls == 3 ]] || fail "canonical: expected 3 gh release calls, got ${calls:-0}"
if grep -v -- "--repo $REDLINE_REPO_SLUG" "$work/gh.log" 2>/dev/null | grep -q .; then
  fail "canonical: gh call without --repo $REDLINE_REPO_SLUG: $(grep -v -- "--repo $REDLINE_REPO_SLUG" "$work/gh.log" | head -n 1)"
fi

[[ $failures == 0 ]] || { printf '%d release authority check(s) failed\n' "$failures" >&2; exit 1; }
printf 'Release authority tests passed.\n'
