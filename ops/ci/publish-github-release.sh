#!/usr/bin/env bash
# Publish the release archives in target/packages as the GitHub release of
# $TAG, with the acceptance manifest $RELEASE_ACCEPTANCE
# (release-acceptance.v1.json, from ops/ci/release-acceptance.sh) as one more
# asset. The release-build workflow's publish job runs it from the checkout of
# the tag. ops/ci/tests/release-authority.sh tests it.
set -euo pipefail
: "${TAG:?TAG is required}"
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# Release authority: publish only from the canonical repository, to it by
# name, and only archives whose provenance names it (ops/release/authority.env).
# shellcheck source=ops/release/authority.env
. "$here/../release/authority.env"
# shellcheck source=scripts/release/package-layout.sh
. "$here/../../scripts/release/package-layout.sh"
if [[ ${GITHUB_REPOSITORY_ID:-} != "$REDLINE_REPO_ID" || ${GITHUB_REPOSITORY:-} != "$REDLINE_REPO_SLUG" ]]; then
  printf 'refusing to publish from %s (id %s): RedlineDB releases are published only by %s (id %s)\n' \
    "${GITHUB_REPOSITORY:-an unknown repository}" "${GITHUB_REPOSITORY_ID:-unset}" "$REDLINE_REPO_SLUG" "$REDLINE_REPO_ID" >&2
  exit 1
fi
# The tag names a new release of the version this checkout's crates carry,
# annotated at HEAD, with notes and a changelog section and no release yet.
bash "$here/release-version.sh" check "$TAG"
notes=docs/releases/${TAG%%-rc.*}.md
# Every durability claim tag in README.md and docs/ needs a passing receipt
# whose source is this commit, or an ancestor with the same binary inputs
# (docs/manual/durability.md#receipts). Needs cargo when any tag exists.
bash "$here/durability-claim-gate.sh" --root "$PWD" --at "$TAG"

# Exactly one archive and one checksum file per package and platform.
assets=()
for package in redlinedb redline-web redline-testing; do
  for platform in linux-x86_64 linux-arm64 macos-x86_64 macos-arm64; do
    assets+=("$package-$TAG-$platform.tar.gz" "$package-$TAG-$platform.tar.gz.sha256")
  done
done
expected=$(printf '%s\n' "${assets[@]}" | LC_ALL=C sort)
present=$(cd target/packages && find . -type f | sed 's|^\./||' | LC_ALL=C sort)
if [[ $present != "$expected" ]]; then
  printf 'target/packages does not hold exactly the release assets of %s:\n' "$TAG" >&2
  diff <(printf '%s\n' "$expected") <(printf '%s\n' "$present") | sed -n 's/^< /  missing: /p; s/^> /  unexpected: /p' >&2
  exit 1
fi
for checksum in target/packages/*.sha256; do
  archive=${checksum##*/}
  archive=${archive%.sha256}
  read -r _ named < "$checksum" || true
  [[ ${named#\*} == "$archive" ]] ||
    { printf '%s does not name %s\n' "${checksum##*/}" "$archive" >&2; exit 1; }
done
(cd target/packages; for checksum in *.sha256; do sha256sum -c "$checksum"; done)
for asset in "${assets[@]}"; do
  [[ $asset == *.tar.gz ]] || continue
  archive=target/packages/$asset
  # Each package's own record (scripts/release/package-layout.sh).
  provenance=$(archive_provenance "$archive") ||
    { printf '%s has no build provenance\n' "$asset" >&2; exit 1; }
  grep -Eq "\"repository_id\":${REDLINE_REPO_ID}[,}]" <<< "$provenance" ||
    { printf '%s was not built by %s\n' "$asset" "$REDLINE_REPO_URL" >&2; exit 1; }
  grep -Fq "\"tag\":\"$TAG\"" <<< "$provenance" ||
    { printf '%s provenance does not name %s\n' "$asset" "$TAG" >&2; exit 1; }
done

# The acceptance manifest must bind exactly these archives, this commit and a
# successful acceptance run (scripts/release/verify-acceptance.sh), with the
# receipts' digests checked when RELEASE_ACCEPTANCE_RECEIPTS names them.
acceptance=${RELEASE_ACCEPTANCE:-}
if [[ -z $acceptance || ${acceptance##*/} != release-acceptance.v1.json || ! -f $acceptance ]]; then
  printf 'RELEASE_ACCEPTANCE must name the release-acceptance.v1.json of %s (ops/ci/release-acceptance.sh)\n' "$TAG" >&2
  exit 1
fi
verify_args=(--packages target/packages --tag "$TAG")
[[ -z ${RELEASE_ACCEPTANCE_RECEIPTS:-} ]] || verify_args+=(--receipts "$RELEASE_ACCEPTANCE_RECEIPTS")
bash "$here/../../scripts/release/verify-acceptance.sh" "$acceptance" "${verify_args[@]}"

# create fails when the release already exists; immutable assets are never clobbered.
args=(--repo "$REDLINE_REPO_SLUG" --verify-tag --draft --title "RedlineDB $TAG" --notes-file "$notes")
[[ $TAG != *-rc.* ]] || args+=(--prerelease)
gh release create "$TAG" "${args[@]}"
gh release upload "$TAG" --repo "$REDLINE_REPO_SLUG" "${assets[@]/#/target/packages/}" "$acceptance"
gh release edit "$TAG" --repo "$REDLINE_REPO_SLUG" --draft=false
