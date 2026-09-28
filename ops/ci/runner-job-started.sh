#!/usr/bin/env bash
# job_started hook for the self-hosted runners (ACTIONS_RUNNER_HOOK_JOB_STARTED;
# ops/ci/install-github-runner.sh installs it). The runner runs the host's
# copy before every job, so a pull request cannot change what it does, and a
# non-zero exit fails the job before any step runs.
#
# 1. Make leftover workspace files deletable: tests may chmod directories
#    0444, and actions/checkout then fails with EACCES.
# 2. Refuse any job whose event carries a pull request from another
#    repository. The workflows already send fork pull requests to
#    GitHub-hosted runners, but a fork can edit the workflow it runs, so the
#    host checks as well. When it cannot tell, it refuses.
#
# See docs/ci-trust-boundary.md.
set -uo pipefail

if [[ -n ${GITHUB_WORKSPACE:-} && -d $GITHUB_WORKSPACE ]]; then
  chmod -R u+rwx "$GITHUB_WORKSPACE" 2>/dev/null
fi
if [[ -n ${RUNNER_TOOL_CACHE:-} && -d $RUNNER_TOOL_CACHE/redlinedb-target ]]; then
  chmod -R u+rwx "$RUNNER_TOOL_CACHE/redlinedb-target" 2>/dev/null
fi

refuse() {
  printf 'redline self-hosted runner: refusing this %s job: %s. Fork pull requests run on GitHub-hosted runners; see docs/ci-trust-boundary.md.\n' \
    "${GITHUB_EVENT_NAME:-unknown}" "$1" >&2
  exit 1
}

event=${GITHUB_EVENT_PATH:-}
case ${GITHUB_EVENT_NAME:-} in
  pull_request*) ;;
  *)
    # Other events are checked only if the payload carries a pull request.
    if [[ -n $event && -r $event ]] && command -v jq >/dev/null 2>&1 \
      && jq -e 'type == "object" and (has("pull_request") | not)' "$event" >/dev/null 2>&1; then
      exit 0
    fi
    ;;
esac

[[ -n $event && -r $event ]] || refuse "its event payload is missing"
command -v jq >/dev/null 2>&1 || refuse "jq is not installed, so the pull request's source cannot be checked"
[[ -n ${GITHUB_REPOSITORY:-} ]] || refuse "GITHUB_REPOSITORY is not set"
head_repo=$(jq -r '.pull_request.head.repo.full_name // ""' "$event" 2>/dev/null) \
  || refuse "its event payload is not readable JSON"
[[ $head_repo == "$GITHUB_REPOSITORY" ]] \
  || refuse "the pull request comes from ${head_repo:-a deleted repository}, not $GITHUB_REPOSITORY"
exit 0
