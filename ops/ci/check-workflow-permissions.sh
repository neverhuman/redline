#!/usr/bin/env bash
# No job of ci.yml or packages.yml may ask for more token permissions than
# release-build.yml grants its acceptance job, which calls ci.yml (and through
# it packages.yml) as a reusable workflow. GitHub checks a called workflow's
# permissions when the run starts, whatever a job's `if:` says, so one job
# asking for more (such as `contents: write`) makes every tag push fail
# before anything runs. Jobs that need more belong in their own workflow
# (report-merge.yml, release-build.yml's publish job).
#
# Usage: bash ops/ci/check-workflow-permissions.sh [workflow directory]
# The CI preflight runs it; ops/ci/tests/workflow-permissions.sh tests it.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
dir=${1:-$root/.github/workflows}

# Every permissions entry of a workflow, one per line:
#   <line> <job, or - at workflow level> <scope> <level>
# for `permissions:` blocks, `{scope: level, ...}` maps, and the scalar forms
# (read-all, write-all), which are reported with scope `*`.
entries() {
  awk '
    function indent(s) { match(s, /^ */); return RLENGTH }
    function emit(scope, level) { print NR, (job == "" ? "-" : job), scope, level }
    /^[ \t]*(#|$)/ { next }
    in_block && indent($0) <= block_indent { in_block = 0 }
    in_block {
      line = $0; sub(/#.*/, "", line)
      if (match(line, /^ *[a-z-]+: *[a-z-]+ *$/)) {
        split(line, kv, ":"); gsub(/ /, "", kv[1]); gsub(/ /, "", kv[2]); emit(kv[1], kv[2])
      } else { emit("?", "unparsed") }
      next
    }
    /^jobs:/ { in_jobs = 1; job = ""; next }
    /^[^ ]/ { in_jobs = 0; job = "" }
    in_jobs && match($0, /^  [A-Za-z0-9_-]+:/) { job = substr($0, 3, RLENGTH - 3) }
    match($0, /^ *permissions:/) {
      rest = substr($0, RLENGTH + 1); sub(/#.*/, "", rest); gsub(/^ +| +$/, "", rest)
      if (rest == "") { in_block = 1; block_indent = indent($0); next }
      if (rest ~ /^\{.*\}$/) {
        rest = substr(rest, 2, length(rest) - 2)
        n = split(rest, pairs, ",")
        for (i = 1; i <= n; i++) {
          if (pairs[i] ~ /^ *$/) continue
          split(pairs[i], kv, ":"); gsub(/ /, "", kv[1]); gsub(/ /, "", kv[2]); emit(kv[1], kv[2])
        }
      } else { emit("*", rest) }
    }
  ' "$1"
}

rank() { case "$1" in none) echo 0 ;; read) echo 1 ;; write) echo 2 ;; *) echo 9 ;; esac; }

grant=$(entries "$dir/release-build.yml" | awk '$2 == "acceptance" { print $3, $4 }')
[[ -n $grant ]] || { printf 'release-build.yml gives its acceptance job no permissions block\n' >&2; exit 1; }
grant_text=$(awk '{ printf "%s%s: %s", (NR > 1 ? ", " : ""), $1, $2 }' <<< "$grant")
allowed() {
  local scope=$1 level=$2 granted
  granted=$(awk -v s="$scope" '$1 == s { print $2 }' <<< "$grant")
  [[ -n $granted || $(rank "$level") == 0 ]] || return 1
  (($(rank "$level") <= $(rank "${granted:-none}")))
}

violations=0
for workflow in ci.yml packages.yml; do
  [[ -f $dir/$workflow ]] || { printf 'missing workflow %s\n' "$workflow" >&2; exit 1; }
  while read -r line job scope level; do
    if [[ $scope == '*' || $scope == '?' ]] || ! allowed "$scope" "$level"; then
      printf '%s:%s: job %s asks for %s: %s, beyond the acceptance grant (%s)\n' \
        "$workflow" "$line" "$job" "$scope" "$level" "$grant_text" >&2
      violations=$((violations + 1))
    fi
  done < <(entries "$dir/$workflow")
done
((violations == 0)) || { printf '%d permission request(s) exceed what release-build.yml grants ci.yml\n' "$violations" >&2; exit 1; }
printf 'ci.yml and packages.yml stay within the acceptance grant.\n'
