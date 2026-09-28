#!/usr/bin/env bash
# Stub redline-testing runner for the scripts/perf fail-closed fixtures
# (scripts/perf/tests/full-fail-closed.sh). It answers --version and
# `list --suite sqlite_parity --format json` like the real runner, and
# `run ... --output <raw>` writes what STUB_MODE asks for:
#
#   complete                 every case: warmups + measured:1..R, all passed,
#                            completion marker, exit 0
#   tolerated-failure        complete; STUB_FAIL_CASE fails every sample, exit 1
#   unexpected-failure       same records and exit as tolerated-failure; the
#                            fixture names a case the baseline does not list
#   nonzero-without-failure  complete and all passed, but exit 1
#   interrupted              one passed measured record, then SIGKILL (137)
#   empty                    no records, exit 0
#   braces                   the single line `{}`, exit 0
#   truncated                complete records with the last line cut in half,
#                            no marker, exit 0
#   missing-repetition       the last case lacks measured:2; marker; exit 0
#   duplicate-sample         the first case's measured:1 twice; marker; exit 0
#   missing-case             the last case is absent; marker; exit 0
#   tampered-marker          complete, but the marker names another hash
#
# STUB_CASES     space-separated numeric case ids (default "10000 10001 10002")
# STUB_FAIL_CASE numeric id of the failing case (default 10002)
# STUB_ARGS_LOG  when set, each `run` appends its arguments there
#
# Every run also leaves a read-only directory in --tmp-root, as a
# read-only database case does, so the fixtures can check that the perf
# lane still removes its temp root.

set -euo pipefail

cases="${STUB_CASES:-10000 10001 10002}"
fail_case="$(printf '%05d' "${STUB_FAIL_CASE:-10002}")"

case "${1:-}" in
  --version|version)
    echo "redline-testing 1.0.1-stub"
    exit 0
    ;;
  list)
    printf '['
    separator=''
    for id in $cases; do
      printf '%s{"id":%d,"name":"CASE_%d","priority":"P1","profile":"memory"}' "$separator" "$id" "$id"
      separator=','
    done
    printf ']\n'
    exit 0
    ;;
  run)
    shift
    ;;
  *)
    echo "stub-redline-testing: unsupported command: $*" >&2
    exit 64
    ;;
esac

if [ -n "${STUB_ARGS_LOG:-}" ]; then
  printf '%s\n' "$*" >> "$STUB_ARGS_LOG"
fi

out='' reps=1 warmup=0 tmp_root=''
while [ $# -gt 0 ]; do
  case "$1" in
    --output) out="$2"; shift 2 ;;
    --tmp-root) tmp_root="$2"; shift 2 ;;
    --repetitions) reps="$2"; shift 2 ;;
    --warmup) warmup="$2"; shift 2 ;;
    --*) shift 2 ;;
    *) shift ;;
  esac
done
[ -n "$out" ] || { echo "stub-redline-testing: --output is required" >&2; exit 2; }
mkdir -p "$(dirname "$out")"
: > "$out"
rm -f "$out.complete.json"

mode="${STUB_MODE:-complete}"

if [ -n "$tmp_root" ]; then
  mkdir -p "$tmp_root/00193-stub-$$/ro.db"
  : > "$tmp_root/00193-stub-$$/ro.db/data.redline"
  chmod 0555 "$tmp_root/00193-stub-$$/ro.db"
fi

# record <case_id> <role> <sample_index> <repetition_index|null> <status>
record() {
  local verdict=passed
  if [ "$5" = failed ]; then verdict=target_semantic_failure; fi
  printf '{"case_id":"%s","name":"CASE_%s","status":"%s","verdict_reason":"%s","sample_role":"%s","sample_index":%s,"repetition_index":%s,"reference_elapsed_ns":1000,"target_elapsed_ns":900,"latency_ratio":0.9}\n' \
    "$1" "$1" "$5" "$verdict" "$2" "$3" "$4"
}

write_marker() {
  local sha="$1"
  local records case_count
  records="$(wc -l < "$out" | tr -d ' ')"
  case_count="$(sed -n 's/^{"case_id":"\([0-9]*\)".*/\1/p' "$out" | sort -u | wc -l | tr -d ' ')"
  printf '{"schema_version":"redline-testing-raw-complete-v1","suite":"sqlite_parity","raw_file":"%s","records":%s,"cases":%s,"raw_sha256":"%s"}\n' \
    "$(basename "$out")" "$records" "$case_count" "$sha" > "$out.complete.json"
}

last_case="${cases##* }"
first_case="${cases%% *}"
for id in $cases; do
  case_id="$(printf '%05d' "$id")"
  status=passed
  case "$mode" in
    tolerated-failure|unexpected-failure)
      if [ "$case_id" = "$fail_case" ]; then status=failed; fi
      ;;
    missing-case)
      if [ "$id" = "$last_case" ]; then continue; fi
      ;;
  esac
  for ((index = 0; index < warmup; index++)); do
    record "$case_id" warmup "$index" null "$status" >> "$out"
  done
  for ((rep = 1; rep <= reps; rep++)); do
    if [ "$mode" = missing-repetition ] && [ "$id" = "$last_case" ] && [ "$rep" = 2 ]; then
      continue
    fi
    record "$case_id" "measured:$rep" "$((warmup + rep - 1))" "$rep" "$status" >> "$out"
    if [ "$mode" = duplicate-sample ] && [ "$id" = "$first_case" ] && [ "$rep" = 1 ]; then
      record "$case_id" "measured:$rep" "$((warmup + rep - 1))" "$rep" "$status" >> "$out"
    fi
    if [ "$mode" = interrupted ]; then
      kill -KILL $$
    fi
  done
done

case "$mode" in
  empty)
    : > "$out"
    exit 0
    ;;
  braces)
    printf '{}\n' > "$out"
    exit 0
    ;;
  truncated)
    size="$(wc -c < "$out")"
    truncate -s "$((size - 40))" "$out"
    exit 0
    ;;
  tampered-marker)
    write_marker "0000000000000000000000000000000000000000000000000000000000000000"
    exit 0
    ;;
esac

write_marker "$(sha256sum "$out" | awk '{print $1}')"
case "$mode" in
  tolerated-failure|unexpected-failure|nonzero-without-failure)
    echo "stub-redline-testing: sqlite_parity failed cases present" >&2
    exit 1
    ;;
esac
exit 0
