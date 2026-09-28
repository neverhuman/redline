#!/usr/bin/env bash
set -euo pipefail
: "${TAG:?TAG is required}"
# Release authority: publish only from the canonical repository, to it by
# name, and only archives whose provenance names it (ops/release/authority.env).
# shellcheck source=ops/release/authority.env
. "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)/ops/release/authority.env"
if [[ ${GITHUB_REPOSITORY_ID:-} != "$REDLINE_REPO_ID" || ${GITHUB_REPOSITORY:-} != "$REDLINE_REPO_SLUG" ]]; then
  printf 'refusing to publish from %s (id %s): RedlineDB releases are published only by %s (id %s)\n' \
    "${GITHUB_REPOSITORY:-an unknown repository}" "${GITHUB_REPOSITORY_ID:-unset}" "$REDLINE_REPO_SLUG" "$REDLINE_REPO_ID" >&2
  exit 1
fi
[[ $TAG =~ ^v4\.1\.0(-rc\.[0-9]+)?$ ]] || { echo 'unsupported release tag' >&2; exit 1; }
[[ $(git rev-parse "$TAG^{commit}") == $(git rev-parse HEAD) ]]
# Every durability claim tag in README.md and docs/ needs a passing receipt
# whose source is this commit, or an ancestor with the same binary inputs
# (docs/manual/durability.md#receipts). Needs cargo when any tag exists.
bash ops/ci/durability-claim-gate.sh --at "$TAG"
count=$(find target/packages -name '*.tar.gz' | wc -l)
[[ $count -eq 12 ]] || { echo 'expected three packages for each of four platforms' >&2; exit 1; }
(cd target/packages; for checksum in *.sha256; do sha256sum -c "$checksum"; done)
for archive in target/packages/*.tar.gz; do
  provenance=$(tar -xzOf "$archive" ./share/redlinedb/build-provenance.json) ||
    { printf '%s has no build provenance\n' "${archive##*/}" >&2; exit 1; }
  grep -Eq "\"repository_id\":${REDLINE_REPO_ID}[,}]" <<< "$provenance" ||
    { printf '%s was not built by %s\n' "${archive##*/}" "$REDLINE_REPO_URL" >&2; exit 1; }
  grep -Fq "\"tag\":\"$TAG\"" <<< "$provenance" ||
    { printf '%s provenance does not name %s\n' "${archive##*/}" "$TAG" >&2; exit 1; }
done
# create fails when the release already exists; immutable assets are never clobbered.
args=(--repo "$REDLINE_REPO_SLUG" --verify-tag --draft --title "RedlineDB $TAG" --notes-file docs/migration/RELEASE_NOTES.md)
[[ $TAG != *-rc.* ]] || args+=(--prerelease)
gh release create "$TAG" "${args[@]}"
gh release upload "$TAG" --repo "$REDLINE_REPO_SLUG" target/packages/*.tar.gz target/packages/*.sha256
gh release edit "$TAG" --repo "$REDLINE_REPO_SLUG" --draft=false
