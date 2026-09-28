#!/usr/bin/env bash
# Full sqlite_parity corpus through the in-tree redline-testing runner:
# 1 warmup and 3 measured repetitions of every case the runner lists
# (`redline-testing list --suite sqlite_parity`; 2,445 cases in runner
# 1.0.1), with SQLite and RedlineDB alternating which runs first in each
# sample (PERF_ORDER, default alternate).
#
# Workers: PERF_WORKERS, default 10. CPU pinning only when PERF_TASKSET_CPUS
# is set. Parallel case workers contend for the host and inflate both
# engines' timings, RedlineDB's more, so a timing claim needs
# PERF_WORKERS=1 pinned to idle cores (see scripts/perf/lib.sh).
#
# Fails closed. Each invocation gets its own run directory,
#   $PERF_ROOT/runs/<UTC timestamp>-<pid>-<output-name>/
# holding cases.json (the runner's corpus listing), build-contract.json
# (the measured binaries and, as PERF_BUILD_* declare it, how the target was
# built; see scripts/perf/lib.sh), sqlite_parity.jsonl with the runner's
# completion marker and evidence, validation.txt and summary.txt. Nothing is
# written under a shared name. The run passes only when:
#   1. perf_evidence validate-run finds exactly the listed cases, each
#      executed case with 1 warmup and measured:1..3, each sample once, and
#      the runner's completion marker certifying the file;
#   2. every failed case is one metadata/sqlite_parity/known-failures.json
#      lists (PERF_KNOWN_FAILURES overrides it), failing at its listed stage
#      (scripts/parity-tolerate-known-optional.sh, on this run's file only);
#   3. the runner exited 0, or it exited non-zero and the complete run holds
#      at least one failed case, all of them listed. This lane gives the
#      runner no baseline, so the runner fails any run with a failed case.
# Then perf_evidence summarize-jsonl --expected-repetitions 3 prints the
# case-level summary. Anything else exits non-zero and keeps the run
# directory for diagnosis; a runner that died keeps its exit code.
#
# Usage: scripts/perf/full.sh <target-binary> <output-name>
#   PERF_RUN_DIR_OUT=<file>  also write the run directory's path to <file>

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$(git -C "$script_dir" rev-parse --show-toplevel)"
source "$script_dir/lib.sh"

target_bin="${1:?usage: full.sh <target-binary> <output-name>}"
out_name="${2:?usage: full.sh <target-binary> <output-name>}"
case "$out_name" in
  ''|.|..|*/*)
    printf 'perf-full: output name must be a plain file name, got %s\n' "$out_name" >&2
    exit 2
    ;;
esac
reps=3
warmup=1

perf_require_bins "$target_bin"

mkdir -p "$PERF_ROOT/runs"
run_id="$(date -u +%Y%m%dT%H%M%SZ)-$$-${out_name}"
run_dir="$PERF_ROOT/runs/$run_id"
# No -p: an existing directory is another run's, never this one's.
mkdir "$run_dir"
if [ -n "${PERF_RUN_DIR_OUT:-}" ]; then
  printf '%s\n' "$run_dir" > "$PERF_RUN_DIR_OUT"
fi
out="$run_dir/sqlite_parity.jsonl"
cases="$run_dir/cases.json"
tmp_tag="full-$run_id"
tmp_root="$(perf_tmp_root "$tmp_tag")"
# Case fixtures can leave read-only directories behind (a read-only database
# case, for one); make them writable before removing this run's temp root,
# and never let the cleanup change the run's exit status.
remove_tmp_root() {
  local status=$?
  chmod -R u+rwX "$tmp_root" 2>/dev/null || true
  if ! rm -rf "$tmp_root" 2>/dev/null; then
    printf 'perf-full: could not remove temp root %s\n' "$tmp_root" >&2
  fi
  exit "$status"
}
trap remove_tmp_root EXIT
printf '==> full.sh: %s -> %s\n' "$target_bin" "$run_dir"

"$REDLINE_TESTING_BIN" list --suite sqlite_parity --format json > "$cases"
expected_cases="$(jq -e 'length' "$cases")"
perf_build_contract "$run_dir/build-contract.json" "$target_bin" > /dev/null

if [ -z "${PERF_TASKSET_CPUS:-}" ]; then
  export PERF_TASKSET_DISABLE=1
fi
run_exit=0
PERF_WORKERS="${PERF_WORKERS:-10}" perf_run_jsonl \
  "$target_bin" "$reps" "$warmup" "$out" "$tmp_tag" || run_exit=$?

fail() {
  local code="$1"
  shift
  printf 'perf-full: %s; run directory kept: %s\n' "$*" "$run_dir" >&2
  exit "$code"
}

if [ ! -s "$out" ]; then
  fail "$(( run_exit == 0 ? 1 : run_exit ))" "redline-testing wrote no records (exit $run_exit)"
fi
if ! perf_evidence validate-run \
  --expected-cases "$expected_cases" \
  --case-manifest "$cases" \
  --reps "$reps" \
  --warmup "$warmup" \
  "$out" > "$run_dir/validation.txt"; then
  fail "$(( run_exit == 0 ? 1 : run_exit ))" "the run is not the complete requested experiment (runner exit $run_exit)"
fi
cat "$run_dir/validation.txt"

tolerate_args=()
if [ "$run_exit" -ne 0 ]; then
  tolerate_args=(--expect-failures)
fi
if ! bash "$script_dir/../parity-tolerate-known-optional.sh" "${tolerate_args[@]}" \
  "$out" "$PERF_KNOWN_FAILURES"; then
  fail "$(( run_exit == 0 ? 1 : run_exit ))" "failures outside the known-failures baseline, or a runner exit ($run_exit) no failure explains"
fi

if ! perf_summarize_jsonl --expected-repetitions "$reps" "$out" > "$run_dir/summary.txt"; then
  fail 1 "no publishable case-level summary"
fi
printf '\n== summary ==\n'
cat "$run_dir/summary.txt"
printf 'wrote %s\n' "$run_dir"
