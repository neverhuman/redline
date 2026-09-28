#!/usr/bin/env bash
# Write release-acceptance.v1.json: what the release of $TAG was accepted on
# (CI-04/CI-06). release-build.yml's publish job runs it from the checkout of
# the tag, after downloading the package archives and the receipt artifacts of
# the same run, then checks the result with scripts/release/verify-acceptance.sh,
# attests it and uploads it as a release asset.
#
#   ops/ci/release-acceptance.sh <packages dir> <receipts dir> <output file>
#
# The manifest (schema redline.release-acceptance/v1) records:
#   repository {slug, id}   the canonical repository (ops/release/authority.env)
#   tag, commit, tree       the tag, the commit it names (HEAD) and its tree
#   source_inputs_sha256    ops/ci/source-inputs-sha256.sh at HEAD
#   clean                   whether `git status --porcelain` was empty
#   rustc                   the compiler every package's provenance names
#   run {id, attempt, url}  this workflow run attempt
#   jobs [{name, conclusion}]  its completed jobs (gh api
#                              .../runs/<id>/attempts/<attempt>/jobs)
#   pending_jobs [name]     jobs not completed yet (publish itself)
#   packages [{name, sha256}]  every archive in <packages dir>
#   receipts [{name, sha256, files}]  every artifact ops/release/acceptance-receipts
#                           names: <receipts dir>/<name>/, digested as the
#                           sha256 of its sorted `sha256sum ./<path>` lines
#
# It refuses, writing nothing, when it runs outside the canonical repository,
# when the tag does not name HEAD, when an archive disagrees with its checksum
# file or has no readable provenance, when the packages name different
# compilers, and when a receipt is missing or empty. Judging the facts (every
# job succeeded, the tree is clean, the digests and provenance match) is
# verify-acceptance.sh's job. ops/ci/tests/release-acceptance.sh tests both.
#
# Environment: TAG, GITHUB_REPOSITORY, GITHUB_REPOSITORY_ID, GITHUB_RUN_ID,
# GITHUB_RUN_ATTEMPT, GITHUB_SERVER_URL, and GH_TOKEN (actions: read) for gh.
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$here/../.." && pwd)
# shellcheck source=ops/release/authority.env
. "$root/ops/release/authority.env"
# shellcheck source=scripts/release/package-layout.sh
. "$root/scripts/release/package-layout.sh"

die() {
  printf 'release-acceptance.sh: %s\n' "$*" >&2
  exit 1
}
[[ $# == 3 ]] || die "usage: release-acceptance.sh <packages dir> <receipts dir> <output file>"
packages_dir=$1 receipts_dir=$2 output=$3
: "${TAG:?TAG is required}" "${GITHUB_RUN_ID:?GITHUB_RUN_ID is required}" "${GITHUB_RUN_ATTEMPT:?GITHUB_RUN_ATTEMPT is required}"
server=${GITHUB_SERVER_URL:-https://github.com}

if [[ ${GITHUB_REPOSITORY_ID:-} != "$REDLINE_REPO_ID" || ${GITHUB_REPOSITORY:-} != "$REDLINE_REPO_SLUG" ]]; then
  die "refusing to accept a release of ${GITHUB_REPOSITORY:-an unknown repository} (id ${GITHUB_REPOSITORY_ID:-unset}): RedlineDB releases come only from $REDLINE_REPO_SLUG (id $REDLINE_REPO_ID)"
fi
[[ $TAG =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[1-9][0-9]*)?$ ]] || die "$TAG is not a release tag"
[[ $GITHUB_RUN_ID =~ ^[1-9][0-9]*$ && $GITHUB_RUN_ATTEMPT =~ ^[1-9][0-9]*$ ]] || die "bad run id or attempt"

commit=$(git rev-parse --verify 'HEAD^{commit}')
tagged=$(git rev-parse --verify --quiet "refs/tags/$TAG^{commit}") || die "tag $TAG is not in this checkout"
[[ $tagged == "$commit" ]] || die "tag $TAG names $tagged, but the checkout is at $commit"
tree=$(git rev-parse --verify 'HEAD^{tree}')
inputs=$(bash "$here/source-inputs-sha256.sh")
clean=true
[[ -z $(git status --porcelain --untracked-files=normal) ]] || clean=false

sha256() { sha256sum "$1" | cut -d' ' -f1; }

# --- packages -----------------------------------------------------------------
[[ -d $packages_dir ]] || die "no packages directory $packages_dir"
shopt -s nullglob
archives=("$packages_dir"/*.tar.gz)
shopt -u nullglob
((${#archives[@]})) || die "no package archives in $packages_dir"
packages_json='[]' compilers=''
for archive in "${archives[@]}"; do
  name=${archive##*/}
  digest=$(sha256 "$archive")
  [[ -f $archive.sha256 ]] || die "$name has no checksum file"
  read -r listed _ < "$archive.sha256" || true
  [[ $listed == "$digest" ]] || die "$name does not match its checksum file ($digest, file says ${listed:-nothing})"
  rust=$(archive_provenance "$archive" | jq -er '.rust') \
    || die "$name has no readable build provenance"
  compilers+=$rust$'\n'
  packages_json=$(jq -c --arg name "$name" --arg sha "$digest" '. + [{name: $name, sha256: $sha}]' <<< "$packages_json")
done
rustc=$(sort -u <<< "${compilers%$'\n'}")
[[ $rustc != *$'\n'* ]] || die "the packages name different compilers: $(tr '\n' ';' <<< "$rustc")"

# --- receipts -------------------------------------------------------------------
# The sha256 of the sorted "<sha256>  ./<path>" lines of every file in a
# directory; verify-acceptance.sh computes the same.
receipt_digest() {
  (cd "$1" && find . -type f | LC_ALL=C sort | while IFS= read -r file; do
    printf '%s  %s\n' "$(sha256 "$file")" "$file"
  done) | sha256sum | cut -d' ' -f1
}
mapfile -t required < <(sed 's/#.*//' "$root/ops/release/acceptance-receipts" | awk 'NF { print $1 }')
missing=() receipts_json='[]'
for name in "${required[@]}"; do
  dir=$receipts_dir/$name
  files=0
  [[ ! -d $dir ]] || files=$(find "$dir" -type f | wc -l)
  if ((files == 0)); then
    missing+=("$name")
    continue
  fi
  receipts_json=$(jq -c --arg name "$name" --arg sha "$(receipt_digest "$dir")" --argjson files "$files" \
    '. + [{name: $name, sha256: $sha, files: $files}]' <<< "$receipts_json")
done
((${#missing[@]} == 0)) || die "missing receipt artifact(s): ${missing[*]} (ops/release/acceptance-receipts names the job that uploads each)"

# --- jobs -------------------------------------------------------------------------
endpoint="repos/$REDLINE_REPO_SLUG/actions/runs/$GITHUB_RUN_ID/attempts/$GITHUB_RUN_ATTEMPT/jobs?per_page=100"
listing=$(gh api --paginate "$endpoint") || die "cannot list the jobs of run $GITHUB_RUN_ID attempt $GITHUB_RUN_ATTEMPT"
all_jobs=$(jq -sc '[.[].jobs[] | {name, status, conclusion}]' <<< "$listing") || die "unreadable job list"

mkdir -p "$(dirname "$output")"
jq -n --arg slug "$REDLINE_REPO_SLUG" --argjson id "$REDLINE_REPO_ID" --arg tag "$TAG" \
  --arg commit "$commit" --arg tree "$tree" --arg inputs "$inputs" --argjson clean "$clean" \
  --arg rustc "$rustc" --argjson run_id "$GITHUB_RUN_ID" --argjson attempt "$GITHUB_RUN_ATTEMPT" \
  --arg url "$server/$REDLINE_REPO_SLUG/actions/runs/$GITHUB_RUN_ID/attempts/$GITHUB_RUN_ATTEMPT" \
  --argjson jobs "$all_jobs" --argjson packages "$packages_json" --argjson receipts "$receipts_json" \
  '{schema: "redline.release-acceptance/v1",
    repository: {slug: $slug, id: $id},
    tag: $tag, commit: $commit, tree: $tree,
    source_inputs_sha256: $inputs, clean: $clean, rustc: $rustc,
    run: {id: $run_id, attempt: $attempt, url: $url},
    jobs: [$jobs[] | select(.status == "completed") | {name, conclusion}],
    pending_jobs: [$jobs[] | select(.status != "completed") | .name],
    packages: ($packages | sort_by(.name)),
    receipts: $receipts}' > "$output.partial"
mv -f "$output.partial" "$output"
printf 'wrote %s: %s at %s, %d jobs, %d packages, %d receipts\n' "$output" "$TAG" "$commit" \
  "$(jq '.jobs | length' "$output")" "$(jq '.packages | length' "$output")" "$(jq '.receipts | length' "$output")"
