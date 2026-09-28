#!/usr/bin/env bash
# Known-failures filter for one `redline-testing run --suite sqlite_parity`
# raw file.
#
# Exit 0 when every failed case in the file is a case the known-failures
# baseline lists for sqlite_parity, failing with the verdict_reason the
# baseline records as its stage. Exit 1 when any other case failed, when
# a listed case failed another way, or when a file cannot be read.
# Tolerance runs one way: a listed case that passes is fine here. The
# official lane's runner gate is the one that ratchets the baseline.
#
# Both paths are explicit. The filter reads exactly the file it is given.
# There is no directory search and no fallback to an all.jsonl or a shared
# sqlite_parity.raw.jsonl, so evidence from another run can never stand in
# for this one. It judges failures only. Whether the run is complete is
# `perf_evidence validate-run`'s question, and callers ask it first.
#
# The baseline is metadata/sqlite_parity/known-failures.json, the reviewed
# list the official lane gives the runner. It replaced the v0.1.2-era
# regexes (00093-00096 optional virtual tables, 00220) and the emptied
# v1.0.1 feature-gap list, which predate the in-tree runner.
#
# --expect-failures: the runner exited non-zero, which in a lane without a
# runner baseline means it saw failed cases. The file must then hold at
# least one, all tolerated; a clean file cannot explain the exit.
#
# Usage: parity-tolerate-known-optional.sh [--expect-failures] <raw.jsonl> <known-failures.json>

set -euo pipefail

usage() {
    printf 'usage: %s [--expect-failures] <raw.jsonl> <known-failures.json>\n' "$0" >&2
    exit 64
}

expect_failures=0
if [ "${1:-}" = "--expect-failures" ]; then
    expect_failures=1
    shift
fi
[ "$#" -eq 2 ] || usage
raw="$1"
baseline="$2"

if [ ! -f "$raw" ] || [ ! -s "$raw" ]; then
    printf 'parity tolerance: %s is not a non-empty raw file\n' "$raw" >&2
    exit 1
fi
if ! jq -e '.schema_version == "redline-testing-sqlite-known-failures-v1" and (.failures | type == "array")' \
    "$baseline" >/dev/null 2>&1; then
    printf 'parity tolerance: %s is not a known-failures baseline (redline-testing-sqlite-known-failures-v1)\n' \
        "$baseline" >&2
    exit 1
fi

declare -A listed_stage=()
while IFS=$'\t' read -r case_id stage; do
    listed_stage["$case_id"]="$stage"
done < <(jq -r '.failures[] | select(.suite == "sqlite_parity") | "\(.case_id)\t\(.stage)"' "$baseline")

# One line per distinct (case, name, verdict_reason) among failed records.
if ! failed_rows="$(jq -r 'select(.status == "failed") | "\(.case_id)\t\(.name)\t\(.verdict_reason // "unrecorded")"' "$raw")"; then
    printf 'parity tolerance: %s is not valid JSONL\n' "$raw" >&2
    exit 1
fi
failed_rows="$(printf '%s' "$failed_rows" | sort -u)"

declare -A tolerated=()
declare -A unexpected=()
unexpected_lines=""
while IFS=$'\t' read -r case_id name reason; do
    [ -n "$case_id" ] || continue
    stage="${listed_stage[$case_id]:-}"
    if [ -n "$stage" ] && [ "$stage" = "$reason" ]; then
        tolerated["$case_id"]=1
        continue
    fi
    if [ -z "${unexpected[$case_id]:-}" ] && [ "${#unexpected[@]}" -lt 20 ]; then
        if [ -n "$stage" ]; then
            unexpected_lines+="  - ${case_id} (${name}): failed with ${reason}; listed with stage ${stage}"$'\n'
        else
            unexpected_lines+="  - ${case_id} (${name}): failed with ${reason}; not listed"$'\n'
        fi
    fi
    unexpected["$case_id"]=1
done <<<"$failed_rows"

# A case with one listed and one unlisted reason is unexpected, not both.
for case_id in "${!unexpected[@]}"; do
    unset 'tolerated[$case_id]'
done

if [ "${#unexpected[@]}" -gt 0 ]; then
    printf 'parity tolerance: %d failed case(s) in %s are not tolerated by %s:\n' \
        "${#unexpected[@]}" "$raw" "$baseline" >&2
    printf '%s' "$unexpected_lines" >&2
    if [ "${#unexpected[@]}" -gt 20 ]; then
        printf '  ... and %d more\n' "$((${#unexpected[@]} - 20))" >&2
    fi
    exit 1
fi

if [ "$expect_failures" -eq 1 ] && [ "${#tolerated[@]}" -eq 0 ]; then
    printf 'parity tolerance: the runner failed, but %s records no failed case to explain it\n' "$raw" >&2
    exit 1
fi

printf 'parity tolerance: %d failed case(s) in %s, all listed in %s\n' \
    "${#tolerated[@]}" "$raw" "$baseline" >&2
exit 0
