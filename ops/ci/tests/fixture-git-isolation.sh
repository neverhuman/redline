#!/usr/bin/env bash
# Run fixture-repository tests the way a git hook runs them, and prove they
# leave the caller's repository alone.
#
# A pre-push hook in a linked worktree exports GIT_DIR, GIT_WORK_TREE and
# GIT_INDEX_FILE, and `git -C <fixture>` does not override them. A test that
# does not clear them commits, tags and resets in the repository being pushed
# instead of its fixture. Each script named here runs with those variables
# pointing at a throwaway "caller" repository and must pass; afterwards that
# repository's refs, index and worktree must be unchanged. The scripts'
# output is shown as they run.
#
#   ops/ci/tests/fixture-git-isolation.sh <test script>...
set -euo pipefail
while read -r variable; do unset "$variable"; done < <(git rev-parse --local-env-vars)
(($#)) || { printf 'usage: %s <test script>...\n' "$0" >&2; exit 64; }
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

caller=$work/caller
mkdir -p "$caller"
git init --quiet "$caller"
printf 'tracked\n' > "$caller/tracked.txt"
git -C "$caller" add tracked.txt
git -C "$caller" -c user.name=caller -c user.email=caller@example.invalid -c commit.gpgSign=false \
  commit --quiet -m caller
git -C "$caller" tag caller-tag
state() {
  git -C "$caller" for-each-ref --format='%(refname) %(objectname)'
  git -C "$caller" rev-parse HEAD
  git -C "$caller" ls-files --stage
  git -C "$caller" status --porcelain --untracked-files=all
}
before=$(state)

failures=0
for script in "$@"; do
  status=0
  (cd "$root" && env GIT_DIR="$caller/.git" GIT_WORK_TREE="$caller" GIT_INDEX_FILE="$caller/.git/index" \
    bash "$script") || status=$?
  if ((status != 0)); then
    printf 'FAIL: %s exited %d with GIT_DIR pointing at another repository\n' "$script" "$status" >&2
    failures=$((failures + 1))
  fi
  after=$(state)
  if [[ $after != "$before" ]]; then
    printf 'FAIL: %s changed the repository GIT_DIR named:\n%s\n' "$script" \
      "$(diff <(printf '%s\n' "$before") <(printf '%s\n' "$after") || true)" >&2
    failures=$((failures + 1))
    before=$after
  fi
done
((failures == 0)) || exit 1
printf 'fixture git isolation: %d script(s) left the calling repository unchanged\n' "$#"
