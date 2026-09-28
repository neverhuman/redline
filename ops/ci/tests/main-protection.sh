#!/usr/bin/env bash
# Tests for ops/release/main-protection.sh: `check` compares the live settings
# of `main` on the release repository with the policy docs/testing.md
# describes, and `apply` sends that policy. No network: gh is a shim that
# answers from fixtures and logs every call.
#
# Usage: bash ops/ci/tests/main-protection.sh
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
# shellcheck source=ops/release/authority.env
. "$root/ops/release/authority.env"
script=$root/ops/release/main-protection.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }

mkdir -p "$work/bin" "$work/fixture"
# The shim prints fixture/<name>.json for a GET and records a write's body.
cat >"$work/bin/gh" <<EOF
#!/usr/bin/env bash
printf '%s\n' "\$*" >>"$work/gh.log"
[[ \$1 == api ]] || exit 2
shift
method=GET input=
while [[ \$# -gt 0 ]]; do
  case \$1 in
    --method) method=\$2; shift 2 ;;
    --input) input=\$2; shift 2 ;;
    -F|-f) printf '%s\n' "\$2" >>"$work/fields.log"; shift 2 ;;
    *) path=\$1; shift ;;
  esac
done
if [[ \$method != GET ]]; then
  [[ \$input == - ]] && cat >"$work/put-body.json"
  printf '%s %s\n' "\$method" "\$path" >>"$work/writes.log"
  exit 0
fi
case \$path in
  repos/$REDLINE_REPO_SLUG) cat "$work/fixture/repo.json" ;;
  repos/$REDLINE_REPO_SLUG/branches/main/protection)
    if [[ -f $work/fixture/protection.json ]]; then
      cat "$work/fixture/protection.json"
    else
      printf 'gh: Branch not protected (HTTP 404)\n' >&2
      exit 1
    fi ;;
  *) printf 'unexpected path %s\n' "\$path" >&2; exit 3 ;;
esac
EOF
chmod +x "$work/bin/gh"

good_repo() {
  jq -n --argjson id "$REDLINE_REPO_ID" \
    '{id: $id, allow_rebase_merge: true, allow_squash_merge: false, allow_merge_commit: false}' \
    >"$work/fixture/repo.json"
}
good_protection() {
  jq -n '{
    required_status_checks: {strict: true, contexts: ["RedlineDB/required"],
      checks: [{context: "RedlineDB/required", app_id: null}]},
    required_pull_request_reviews: {required_approving_review_count: 1,
      dismiss_stale_reviews: true},
    required_linear_history: {enabled: true},
    enforce_admins: {enabled: true},
    allow_force_pushes: {enabled: false},
    allow_deletions: {enabled: false}
  }' >"$work/fixture/protection.json"
}
# expect_check <label> <exit> [message fragment]
expect_check() {
  local label=$1 want=$2 fragment=${3:-} got=0 output
  output=$(PATH="$work/bin:$PATH" bash "$script" check 2>&1) || got=$?
  [[ $got == "$want" ]] || fail "$label: exit $got, want $want: $output"
  [[ -z $fragment || $output == *"$fragment"* ]] || fail "$label: output lacks '$fragment': $output"
}

good_repo
good_protection
expect_check 'the documented policy' 0

rm "$work/fixture/protection.json"
expect_check 'an unprotected main' 1 'main is not protected'

good_protection
jq '.allow_squash_merge = true' "$work/fixture/repo.json" >"$work/r" && mv "$work/r" "$work/fixture/repo.json"
expect_check 'squash merging allowed' 1 'allow_squash_merge'

good_repo
jq '.required_status_checks.contexts = ["ci"] | .required_status_checks.checks = [{context: "ci"}]' \
  "$work/fixture/protection.json" >"$work/p" && mv "$work/p" "$work/fixture/protection.json"
expect_check 'a missing required context' 1 'RedlineDB/required'

good_protection
jq '.required_linear_history.enabled = false | .required_pull_request_reviews = null' \
  "$work/fixture/protection.json" >"$work/p" && mv "$work/p" "$work/fixture/protection.json"
output=$(PATH="$work/bin:$PATH" bash "$script" check 2>&1) && fail 'no linear history passed'
[[ $output == *required_linear_history* && $output == *required_approving_review_count* ]] ||
  fail "every mismatch is not listed: $output"

jq '.id = 1' "$work/fixture/repo.json" >"$work/r" && mv "$work/r" "$work/fixture/repo.json"
good_protection
expect_check 'another repository behind the slug' 1 'repository id'

# apply writes the policy, then checks it (the shim still serves a good state).
good_repo
: >"$work/writes.log"
PATH="$work/bin:$PATH" bash "$script" apply >/dev/null 2>&1 || fail 'apply failed'
grep -qx "PUT repos/$REDLINE_REPO_SLUG/branches/main/protection" "$work/writes.log" ||
  fail 'apply did not PUT the branch protection'
grep -qx "PATCH repos/$REDLINE_REPO_SLUG" "$work/writes.log" || fail 'apply did not PATCH the merge methods'
jq -e '.required_status_checks.strict == true
  and (.required_status_checks.contexts == ["RedlineDB/required"])
  and .required_pull_request_reviews.required_approving_review_count == 1
  and .required_pull_request_reviews.dismiss_stale_reviews == true
  and .required_linear_history == true and .enforce_admins == true
  and .allow_force_pushes == false and .allow_deletions == false' \
  "$work/put-body.json" >/dev/null || fail "apply sent another policy: $(cat "$work/put-body.json")"
for field in allow_squash_merge=false allow_merge_commit=false allow_rebase_merge=true; do
  grep -qx "$field" "$work/fields.log" || fail "apply did not set $field"
done

# A write never happens in check mode.
: >"$work/writes.log"
PATH="$work/bin:$PATH" bash "$script" check >/dev/null 2>&1 || true
[[ ! -s $work/writes.log ]] || fail "check wrote: $(cat "$work/writes.log")"

if ((failures)); then
  printf 'main-protection: %d failure(s)\n' "$failures" >&2
  exit 1
fi
printf 'main-protection: ok\n'
