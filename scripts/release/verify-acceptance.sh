#!/usr/bin/env bash
# Check a release's acceptance manifest (release-acceptance.v1.json, written
# by ops/ci/release-acceptance.sh) against the release it describes. Fails
# closed: anything missing, unreadable or different is a refusal, and every
# problem found is listed.
#
#   scripts/release/verify-acceptance.sh <manifest> --packages <dir>
#       [--receipts <dir>] [--tag <tag>]
#
# Run it from a checkout of the tagged commit, with the tag fetched:
#
#   git fetch origin tag v5.0.0 && git checkout v5.0.0
#   gh release download v5.0.0 --repo neverhuman/redline --dir release
#   bash scripts/release/verify-acceptance.sh release/release-acceptance.v1.json \
#     --packages release --tag v5.0.0
#
# It requires:
#   - schema redline.release-acceptance/v1 and the canonical repository slug
#     and id (ops/release/authority.env);
#   - a release tag (equal to --tag when given) that this checkout's tag names,
#     the checkout at that commit, the manifest's tree and source-inputs hash
#     (ops/ci/source-inputs-sha256.sh) equal to this checkout's, and clean;
#   - the run URL of that repository, at least one job, every job concluded
#     success, a RedlineDB/required job among them, and no job still running
#     other than publish and verify-published;
#   - exactly the twelve release archives of the tag, each present in
#     --packages with that sha256 (and its checksum file, if present, agreeing),
#     no other archive there, and each archive's build provenance naming the
#     repository id, tag, commit, tree and the manifest's compiler;
#   - exactly the receipts ops/release/acceptance-receipts names, and with
#     --receipts <dir>, each <dir>/<name>/ hashing to its recorded digest and
#     the security receipt passing for the commit.
# release-build.yml runs it before publishing; ops/ci/tests/release-acceptance.sh
# tests it.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
# shellcheck source=ops/release/authority.env
. "$root/ops/release/authority.env"
# shellcheck source=scripts/release/package-layout.sh
. "$root/scripts/release/package-layout.sh"

usage() {
  printf 'usage: verify-acceptance.sh <manifest> --packages <dir> [--receipts <dir>] [--tag <tag>]\n' >&2
  exit 64
}
[[ $# -ge 1 ]] || usage
manifest=$1
shift
packages_dir='' receipts_dir='' want_tag=''
while (($#)); do
  case $1 in
    --packages) packages_dir=${2:?}; shift 2 ;;
    --receipts) receipts_dir=${2:?}; shift 2 ;;
    --tag) want_tag=${2:?}; shift 2 ;;
    *) usage ;;
  esac
done
[[ -n $packages_dir ]] || usage

problems=()
problem() { problems+=("$*"); }
finish() {
  if ((${#problems[@]})); then
    printf 'verify-acceptance: %s is refused:\n' "$manifest" >&2
    printf '  - %s\n' "${problems[@]}" >&2
    exit 1
  fi
}
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}
field() { jq -r "$1" "$manifest"; }

if [[ ! -f $manifest ]] || ! jq -e 'type == "object"' "$manifest" >/dev/null 2>&1; then
  problem "not a JSON manifest"
  finish
fi
[[ $(field .schema) == redline.release-acceptance/v1 ]] || problem "schema is $(field .schema), not redline.release-acceptance/v1"
[[ $(field .repository.slug) == "$REDLINE_REPO_SLUG" && $(field .repository.id) == "$REDLINE_REPO_ID" ]] \
  || problem "repository is $(field .repository.slug) (id $(field .repository.id)), not $REDLINE_REPO_SLUG (id $REDLINE_REPO_ID)"

tag=$(field .tag) commit=$(field .commit) tree=$(field .tree) rustc=$(field .rustc)
[[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[1-9][0-9]*)?$ ]] || problem "tag $tag is not a release tag"
[[ -z $want_tag || $tag == "$want_tag" ]] || problem "tag $tag does not match --tag $want_tag"
[[ $commit =~ ^[0-9a-f]{40}$ && $tree =~ ^[0-9a-f]{40}$ ]] || problem "commit or tree is not a SHA-1"
[[ $(field .clean) == true ]] || problem "the accepted checkout was not clean"
[[ $rustc =~ ^rustc\ [0-9]+\.[0-9]+\.[0-9]+ ]] || problem "rustc is '$rustc'"

# --- the source: this checkout must be the accepted commit ---------------------
head=$(git rev-parse --verify --quiet 'HEAD^{commit}' || true)
if [[ $head != "$commit" ]]; then
  problem "this checkout is at ${head:-no commit}; check out $tag ($commit) to verify its manifest"
else
  tagged=$(git rev-parse --verify --quiet "refs/tags/$tag^{commit}" || true)
  [[ $tagged == "$commit" ]] || problem "tag $tag names ${tagged:-nothing} in this checkout, not $commit"
  actual_tree=$(git rev-parse 'HEAD^{tree}')
  [[ $actual_tree == "$tree" ]] || problem "tree is $tree in the manifest, $actual_tree in the commit"
  inputs=$(bash "$root/ops/ci/source-inputs-sha256.sh")
  [[ $(field .source_inputs_sha256) == "$inputs" ]] \
    || problem "source_inputs_sha256 is $(field .source_inputs_sha256) in the manifest, $inputs in the commit"
fi

# --- the run and its jobs ---------------------------------------------------------
[[ $(field .run.url) == "https://github.com/$REDLINE_REPO_SLUG/actions/runs/$(field .run.id)/attempts/$(field .run.attempt)" ]] \
  || problem "run url $(field .run.url) is not a run of $REDLINE_REPO_SLUG"
[[ $(jq '.jobs | length' "$manifest") -gt 0 ]] || problem "no jobs recorded"
while IFS=$'\t' read -r name conclusion; do
  problem "job $name: $conclusion, not success"
done < <(jq -r '.jobs[] | select(.conclusion != "success") | [.name, (.conclusion // "none")] | @tsv' "$manifest")
jq -e 'any(.jobs[]; (.name | endswith("RedlineDB/required")))' "$manifest" >/dev/null \
  || problem "no RedlineDB/required job among the jobs"
while IFS= read -r name; do
  problem "still running: $name"
done < <(jq -r '.pending_jobs[] | select(. != "publish" and (startswith("verify-published") | not))' "$manifest")

# --- packages -----------------------------------------------------------------------
expected=()
for package in redlinedb redline-web redline-testing; do
  for platform in linux-x86_64 linux-arm64 macos-x86_64 macos-arm64; do
    expected+=("$package-$tag-$platform.tar.gz")
  done
done
listed=$(jq -r '.packages[].name' "$manifest" | LC_ALL=C sort)
wanted=$(printf '%s\n' "${expected[@]}" | LC_ALL=C sort)
if [[ $listed != "$wanted" ]]; then
  problem "manifest packages differ from the release archives of $tag: $(diff <(printf '%s\n' "$wanted") <(printf '%s\n' "$listed") | sed -n 's/^< /missing: /p; s/^> /unexpected: /p' | tr '\n' ' ')"
fi
if [[ ! -d $packages_dir ]]; then
  problem "no packages directory $packages_dir"
else
  while IFS=$'\t' read -r name digest; do
    archive=$packages_dir/$name
    if [[ ! -f $archive ]]; then
      problem "$name: not in $packages_dir"
      continue
    fi
    actual=$(sha256 "$archive")
    [[ $actual == "$digest" ]] || problem "$name: sha256 $actual is not the manifest's $digest"
    if [[ -f $archive.sha256 ]]; then
      read -r sidecar _ < "$archive.sha256" || true
      [[ $sidecar == "$digest" ]] || problem "$name: its checksum file says ${sidecar:-nothing}"
    fi
    provenance=$(archive_provenance "$archive" 2>/dev/null) \
      || { problem "$name: no build provenance"; continue; }
    jq -e --argjson id "$REDLINE_REPO_ID" --arg tag "$tag" --arg commit "$commit" --arg tree "$tree" --arg rust "$rustc" \
      '.repository_id == $id and .tag == $tag and .commit == $commit and .source_tree == $tree and .rust == $rust' \
      <<< "$provenance" >/dev/null \
      || problem "$name: provenance names $(jq -c '{repository_id, tag, commit, source_tree, rust}' <<< "$provenance"), not the manifest's"
  done < <(jq -r '.packages[] | [.name, .sha256] | @tsv' "$manifest")
  for archive in "$packages_dir"/*.tar.gz; do
    [[ -f $archive ]] || continue
    jq -e --arg name "${archive##*/}" 'any(.packages[]; .name == $name)' "$manifest" >/dev/null \
      || problem "archive not in the manifest: ${archive##*/}"
  done
fi

# --- receipts -----------------------------------------------------------------------
required=$(sed 's/#.*//' "$root/ops/release/acceptance-receipts" | awk 'NF { print $1 }')
recorded=$(jq -r '.receipts[].name' "$manifest")
[[ $recorded == "$required" ]] \
  || problem "receipts differ from ops/release/acceptance-receipts: recorded [$(tr '\n' ' ' <<< "$recorded")], required [$(tr '\n' ' ' <<< "$required")]"
while IFS=$'\t' read -r name digest files; do
  [[ $digest =~ ^[0-9a-f]{64}$ && $files -ge 1 ]] || problem "receipt $name: no digest or no files"
  [[ -n $receipts_dir ]] || continue
  dir=$receipts_dir/$name
  if [[ ! -d $dir ]]; then
    problem "receipt $name: not in $receipts_dir"
    continue
  fi
  actual=$(cd "$dir" && find . -type f | LC_ALL=C sort | while IFS= read -r file; do
    printf '%s  %s\n' "$(sha256 "$file")" "$file"
  done | { if command -v sha256sum >/dev/null 2>&1; then sha256sum; else shasum -a 256; fi; } | cut -d' ' -f1)
  [[ $actual == "$digest" ]] || problem "receipt $name: sha256 $actual is not the manifest's $digest"
done < <(jq -r '.receipts[] | [.name, .sha256, (.files | tostring)] | @tsv' "$manifest")
if [[ -n $receipts_dir ]]; then
  security=$receipts_dir/security-receipt/receipt.json
  if [[ ! -f $security ]]; then
    problem "security-receipt: no receipt.json"
  else
    [[ $(jq -r '.candidate.sha' "$security") == "$commit" ]] \
      || problem "security-receipt: receipt.json is for $(jq -r '.candidate.sha' "$security"), not $commit"
    [[ $(jq -r '.result' "$security") == pass && $(jq -r '.candidate.dirty' "$security") == false && $(jq -r '.candidate.shallow' "$security") == false ]] \
      || problem "security-receipt: result $(jq -r '.result' "$security"), dirty $(jq -r '.candidate.dirty' "$security"), shallow $(jq -r '.candidate.shallow' "$security")"
  fi
fi

finish
printf 'release-acceptance.v1.json verified: %s at %s, %s jobs succeeded, %s packages, %s receipts%s.\n' \
  "$tag" "$commit" "$(jq '.jobs | length' "$manifest")" "$(jq '.packages | length' "$manifest")" \
  "$(jq '.receipts | length' "$manifest")" "${receipts_dir:+ (digests checked)}"
