#!/usr/bin/env bash
# Check or apply the protection of `main` on the release repository.
#
#   bash ops/release/main-protection.sh check   # read-only; exit 1 lists every mismatch
#   bash ops/release/main-protection.sh apply   # a repository admin only; then checks
#
# The policy (docs/testing.md#publication-and-review):
#   - required status check RedlineDB/required, strict (up to date with main);
#   - one approving review, dismissed when a new commit is pushed;
#   - linear history, enforced for admins, no force-push, no deletion;
#   - one merge method: rebase (squash and merge commits disabled).
# The repository is named by ops/release/authority.env and checked by id.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
# shellcheck source=ops/release/authority.env
. "$root/ops/release/authority.env"
repo=$REDLINE_REPO_SLUG
branch=main
context=RedlineDB/required

die() {
  printf 'main-protection: %s\n' "$*" >&2
  exit 1
}

check() {
  local repo_json protection problems
  repo_json=$(gh api "repos/$repo") || die "cannot read repos/$repo"
  problems=$(jq -r --argjson id "$REDLINE_REPO_ID" '
    (if .id != $id then "repository id is \(.id), want \($id)" else empty end),
    (if .allow_rebase_merge != true then "allow_rebase_merge is \(.allow_rebase_merge), want true" else empty end),
    (if .allow_squash_merge != false then "allow_squash_merge is \(.allow_squash_merge), want false" else empty end),
    (if .allow_merge_commit != false then "allow_merge_commit is \(.allow_merge_commit), want false" else empty end)
  ' <<<"$repo_json")
  if ! protection=$(gh api "repos/$repo/branches/$branch/protection" 2>&1); then
    problems+=${problems:+$'\n'}"$branch is not protected ($protection)"
  else
    problems+=${problems:+$'\n'}$(jq -r --arg context "$context" '
      (if .required_status_checks.strict != true
        then "required_status_checks.strict is not true" else empty end),
      (if ([.required_status_checks.contexts[]?] + [.required_status_checks.checks[]?.context]
           | index($context)) == null
        then "required status check \($context) is missing" else empty end),
      (if (.required_pull_request_reviews.required_approving_review_count // 0) < 1
        then "required_approving_review_count is below 1" else empty end),
      (if .required_pull_request_reviews.dismiss_stale_reviews != true
        then "dismiss_stale_reviews is not true" else empty end),
      (if .required_linear_history.enabled != true
        then "required_linear_history is not enabled" else empty end),
      (if .enforce_admins.enabled != true then "enforce_admins is not enabled" else empty end),
      (if .allow_force_pushes.enabled != false then "force pushes are allowed" else empty end),
      (if .allow_deletions.enabled != false then "deletion is allowed" else empty end)
    ' <<<"$protection")
  fi
  problems=$(sed '/^$/d' <<<"$problems")
  if [[ -n $problems ]]; then
    printf 'main-protection: %s/%s does not match docs/testing.md:\n' "$repo" "$branch" >&2
    sed 's/^/  - /' <<<"$problems" >&2
    return 1
  fi
  printf 'main-protection: %s/%s matches the documented policy\n' "$repo" "$branch"
}

apply() {
  local repo_id
  repo_id=$(gh api "repos/$repo" | jq -r .id) || die "cannot read repos/$repo"
  [[ $repo_id == "$REDLINE_REPO_ID" ]] || die "repos/$repo is id $repo_id, want $REDLINE_REPO_ID"
  jq -n --arg context "$context" '{
    required_status_checks: {strict: true, contexts: [$context]},
    enforce_admins: true,
    required_pull_request_reviews: {required_approving_review_count: 1,
      dismiss_stale_reviews: true},
    restrictions: null,
    required_linear_history: true,
    allow_force_pushes: false,
    allow_deletions: false
  }' | gh api --method PUT "repos/$repo/branches/$branch/protection" --input - >/dev/null
  gh api --method PATCH "repos/$repo" \
    -F allow_squash_merge=false -F allow_merge_commit=false -F allow_rebase_merge=true >/dev/null
  check
}

case ${1:-} in
  check) check ;;
  apply) apply ;;
  *) die "usage: main-protection.sh check|apply" ;;
esac
