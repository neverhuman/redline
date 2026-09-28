#!/usr/bin/env bash
# Merge the open automation/sqlite-parity-report pull request at the head
# that ci passed on. .github/workflows/report-merge.yml runs it; GH_TOKEN,
# GH_REPO and TESTED_HEAD (empty on a manual dispatch) come from the job.
set -euo pipefail
branch=automation/sqlite-parity-report
pr=$(gh pr list --head "$branch" --base main --state open --json number,headRefOid \
  --jq '.[] | "\(.number) \(.headRefOid)"' | head -n 1)
[ -n "$pr" ] || { echo "::error::no open pull request from $branch"; exit 1; }
number=${pr% *} head=${pr#* }
if [ -z "$TESTED_HEAD" ]; then
  passed=$(gh api "repos/$GH_REPO/actions/workflows/ci.yml/runs?branch=$branch&event=workflow_dispatch&status=success&head_sha=$head" \
    --jq '.total_count')
  [ "$passed" -gt 0 ] || { echo "::error::ci has not passed on $head, the head of #$number"; exit 1; }
  TESTED_HEAD=$head
fi
gh pr merge "$number" --rebase --delete-branch --match-head-commit "$TESTED_HEAD"
