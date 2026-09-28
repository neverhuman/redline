#!/usr/bin/env bash
# Tests for the self-hosted runner job_started hook,
# ops/ci/runner-job-started.sh (S8-06). The hook must refuse any job whose
# event carries a pull request from another repository, fail closed when it
# cannot tell, and still unlock the workspace for every other job.
#
# Usage: bash ops/ci/tests/runner-job-started.sh
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
hook=$root/ops/ci/runner-job-started.sh
work=$(mktemp -d)
trap 'chmod -R u+rwx "$work" 2>/dev/null; rm -rf "$work"' EXIT
repo=neverhuman/redline

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

pr_event() { # head repository full_name as a JSON value
  printf '{"action":"synchronize","pull_request":{"head":{"repo":{"full_name":%s}}}}\n' "$1"
}
printf '{"ref":"refs/heads/main","head_commit":{"message":"mention \\"pull_request\\""}}\n' >"$work/push.json"
printf '{"inputs":{},"ref":"refs/heads/main"}\n' >"$work/dispatch.json"
printf '{"schedule":"30 6 * * *"}\n' >"$work/schedule.json"
pr_event "\"$repo\"" >"$work/same-repo.json"
pr_event '"someone/redline"' >"$work/fork.json"
pr_event 'null' >"$work/deleted-fork.json"
printf '{"pull_request":{"head":{}}}\n' >"$work/no-head-repo.json"
printf 'not json\n' >"$work/garbage.json"
printf '{"issue":{"pull_request":{"url":"x"}},"comment":{"body":"hi"}}\n' >"$work/issue-comment.json"

# run_hook <event name> <payload path> [VAR=value...]; sets $status.
run_hook() {
  local name=$1 payload=$2
  shift 2
  status=0
  env -i PATH="${HOOK_PATH:-$PATH}" HOME="$work" GITHUB_REPOSITORY="$repo" \
    GITHUB_EVENT_NAME="$name" GITHUB_EVENT_PATH="$payload" GITHUB_WORKSPACE="$work/ws" "$@" \
    /bin/bash "$hook" >"$work/out" 2>&1 || status=$?
}

expect() { # <want: allow|refuse> <event name> <payload> [VAR=value...]
  local want=$1 name=$2 payload=$3
  shift 3
  run_hook "$name" "$payload" "$@"
  case $want in
    allow) [[ $status -eq 0 ]] || fail "$name $(basename "$payload"): refused: $(cat "$work/out")" ;;
    refuse) [[ $status -ne 0 ]] || fail "$name $(basename "$payload"): allowed a job it must refuse" ;;
  esac
}

expect allow push "$work/push.json"
expect allow workflow_dispatch "$work/dispatch.json"
expect allow schedule "$work/schedule.json"
expect allow pull_request "$work/same-repo.json"
expect allow issue_comment "$work/issue-comment.json"

expect refuse pull_request "$work/fork.json"
grep -q 'someone/redline' "$work/out" || fail "fork refusal does not name the fork: $(cat "$work/out")"
expect refuse pull_request_target "$work/fork.json"
expect refuse pull_request_review "$work/fork.json"
expect refuse pull_request "$work/deleted-fork.json"
expect refuse pull_request "$work/no-head-repo.json"
expect refuse pull_request "$work/garbage.json"
expect refuse pull_request "$work/missing.json"
expect refuse pull_request "$work/same-repo.json" GITHUB_REPOSITORY=
# An event name that does not say pull_request but carries one from a fork.
expect refuse workflow_run "$work/fork.json"

# Without jq the hook cannot read the payload, so it refuses pull requests.
mkdir -p "$work/nojq"
ln -s "$(command -v chmod)" "$work/nojq/chmod"
HOOK_PATH=$work/nojq expect refuse pull_request "$work/same-repo.json"

# Every job still gets its leftover read-only workspace unlocked.
mkdir -p "$work/ws/readonly"
chmod 0555 "$work/ws/readonly"
expect allow push "$work/push.json"
[[ -w $work/ws/readonly ]] || fail "the hook did not unlock the workspace"

echo "ok: runner-job-started.sh refuses fork pull requests and fails closed"
