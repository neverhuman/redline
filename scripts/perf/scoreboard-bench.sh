#!/usr/bin/env bash
# Measures scoreboard binaries (scripts/perf/build-scoreboard.sh) into a
# committed bundle, benchmark-results/perf/releases/<bundle>/:
#   bundle.json                       protocol, versions in order (first = baseline)
#   host.json                         CPU, kernel, governor, file systems, runners
#   <label>/build.json                how each version's harness was built
#   <label>/run-<k>-<pair>.jsonl      raw records, one per repetition
#   runs.jsonl                        host load around every version's run
#   summary.json                      derived; `redline-scoreboard summarize --check`
#
# Every version runs every workload in each run, the versions in a rotated
# order, pinned to the same CPUs. SQLite runs beside each version as its
# control group. A version's run waits up to --max-wait-s for a quiet host
# (no CI job, load below --max-loadavg). If it had to start anyway, or a CI
# job ran at any time during it (sampled every 10 s), runs.jsonl says so and
# the summary refuses to publish the bundle.
# The normal pair runs on tmpfs; the strict pair needs a real disk.
#
# Usage: scripts/perf/scoreboard-bench.sh --bundle <name> [--runs 3] [--rows 20000]
#          [--pairs normal,strict] [--reps 5] [--cpus 2-5] [--max-loadavg 16] [--max-wait-s 1800]
#          [--normal-dir /dev/shm/scoreboard] [--strict-dir <dir on disk>]
#          <label>=<binary> ...

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(git -C "$script_dir" rev-parse --show-toplevel)"
source "$script_dir/lib.sh"

die() {
  printf 'scoreboard-bench: %s\n' "$*" >&2
  exit 2
}

bundle="" runs=3 rows=20000 pairs=normal reps=5 cpus=2-5 max_loadavg=16 max_wait_s=1800
normal_dir=/dev/shm/scoreboard strict_dir="$repo_root/target/scoreboard-strict"
labels=() binaries=()
while [ $# -gt 0 ]; do
  case "$1" in
    --bundle) bundle="${2:?}"; shift 2 ;;
    --runs) runs="${2:?}"; shift 2 ;;
    --rows) rows="${2:?}"; shift 2 ;;
    --pairs) pairs="${2:?}"; shift 2 ;;
    --reps) reps="${2:?}"; shift 2 ;;
    --cpus) cpus="${2:?}"; shift 2 ;;
    --max-loadavg) max_loadavg="${2:?}"; shift 2 ;;
    --max-wait-s) max_wait_s="${2:?}"; shift 2 ;;
    --normal-dir) normal_dir="${2:?}"; shift 2 ;;
    --strict-dir) strict_dir="${2:?}"; shift 2 ;;
    *=*) labels+=("${1%%=*}"); binaries+=("${1#*=}"); shift ;;
    *) die "unknown argument $1" ;;
  esac
done
for number in "$runs" "$rows" "$reps" "$max_wait_s"; do
  [[ "$number" =~ ^[1-9][0-9]*$|^0$ ]] || die "not a whole number: $number"
done
[ "$runs" -ge 1 ] && [ "$rows" -ge 10 ] && [ "$reps" -ge 1 ] || die "--runs, --rows and --reps must be at least 1, 10 and 1"
[[ "$max_loadavg" =~ ^[0-9]+([.][0-9]+)?$ ]] || die "not a number: --max-loadavg $max_loadavg"
[ -n "$bundle" ] || die "--bundle is required"
[[ "$bundle" =~ ^[A-Za-z0-9][A-Za-z0-9._+-]*$ ]] || die "bad bundle name $bundle"
[ "${#labels[@]}" -gt 0 ] || die "name at least one <label>=<binary>"
command -v jq >/dev/null || die "jq is missing"
command -v taskset >/dev/null || die "taskset is missing"

out="$repo_root/benchmark-results/perf/releases/$bundle"
[ ! -e "$out" ] || die "$out exists; a bundle is never overwritten"

# Each binary is the release build its build.json describes, under the label
# given here; one harness and one SQLite serve every version, so only the
# engine may differ.
harness="" sqlite=""
declare -A seen_label=()
for i in "${!labels[@]}"; do
  label="${labels[$i]}" bin="${binaries[$i]}"
  [[ "$label" =~ ^[A-Za-z0-9][A-Za-z0-9._+-]*$ ]] || die "bad label '$label'"
  [ "$label" != sqlite ] || die "the label sqlite is reserved for the control group"
  [ -z "${seen_label[$label]:-}" ] || die "label $label is given twice"
  seen_label[$label]=1
  perf_check_release_binary "$label" "$bin"
  build="$(dirname "$bin")/build.json"
  [ -f "$build" ] || die "$label: no build.json beside $bin"
  [ "$(jq -r .label "$build")" = "$label" ] || die "$label: $build describes $(jq -r .label "$build")"
  [ "$(jq -r .binary_sha256 "$build")" = "$(sha256sum "$bin" | awk '{print $1}')" ] \
    || die "$label: $bin is not the binary $build describes"
  tree="$(jq -r .harness_tree "$build")" lib="$(jq -r .engines.sqlite_source_id "$build")"
  [ -z "$harness" ] || [ "$harness" = "$tree" ] || die "$label was built with another harness ($tree, not $harness)"
  [ -z "$sqlite" ] || [ "$sqlite" = "$lib" ] || die "$label links another SQLite ($lib, not $sqlite)"
  harness="$tree" sqlite="$lib"
done

fstype() { findmnt -n -o FSTYPE -T "$1" 2>/dev/null || echo unknown; }
mkdir -p "$normal_dir"
[ "$(fstype "$normal_dir")" = tmpfs ] || die "the normal pair measures no device cost; $normal_dir is not on tmpfs"
case ",$pairs," in *,strict,*)
  mkdir -p "$strict_dir"
  [ "$(fstype "$strict_dir")" != tmpfs ] || die "the strict pair syncs to disk; $strict_dir is on tmpfs"
esac
# Images and work copies live in per-bundle directories this script makes
# and removes; leftovers from another run would be reused, so refuse them.
for dir in "$normal_dir/$bundle" "$strict_dir/$bundle"; do
  [ ! -e "$dir" ] || die "$dir exists: another run of $bundle is going on, or one was killed; check and remove it"
done
finish() {
  local status=$?
  # A host sampler still running when a run failed.
  kill $(jobs -p) 2>/dev/null || true
  rm -rf "${normal_dir:?}/$bundle" "${strict_dir:?}/$bundle"
  if [ "$status" -ne 0 ] && [ -d "$out" ] && [ -z "${measured:-}" ]; then
    rm -rf "$out"
    printf 'scoreboard-bench: failed; removed the partial bundle %s\n' "$out" >&2
  fi
  exit "$status"
}
trap finish EXIT

runner_jobs() {
  local jobs=0
  if command -v docker >/dev/null; then
    jobs=$(docker ps -q --filter label=jope.runner=1 2>/dev/null | wc -l)
  fi
  jobs=$((jobs + $(pgrep -fc 'Runner.Worker' 2>/dev/null || true)))
  echo "$jobs"
}
loadavg() { cut -d' ' -f1 /proc/loadavg; }

# Wait up to --max-wait-s for no CI job and load below --max-loadavg. Sets
# waited_out=true when the host was still busy and the run starts anyway;
# runs.jsonl records it and the summary refuses to publish the bundle.
wait_quiet() {
  local waited=0
  waited_out=false
  while [ "$(runner_jobs)" -gt 0 ] || awk -v l="$(loadavg)" -v m="$max_loadavg" 'BEGIN { exit !(l >= m) }'; do
    if [ "$waited" -ge "$max_wait_s" ]; then
      printf 'scoreboard-bench: host still busy after %ss (runner jobs %s, load %s); measuring anyway, and the bundle will not be publishable\n' \
        "$max_wait_s" "$(runner_jobs)" "$(loadavg)" >&2
      waited_out=true
      return 0
    fi
    sleep 30
    waited=$((waited + 30))
  done
}

# Sample CI jobs and load every 10 s while a run goes on, keeping the
# highest of each in $1, so a job that starts and ends inside a run counts.
sample_host() {
  local most_jobs=0 most_load=0 jobs load
  while :; do
    jobs="$(runner_jobs)" load="$(loadavg)"
    [ "$jobs" -le "$most_jobs" ] || most_jobs="$jobs"
    most_load="$(awk -v a="$most_load" -v b="$load" 'BEGIN { print (b > a) ? b : a }')"
    # Written whole and renamed, so a kill mid-write never leaves a torn
    # or empty sample behind.
    printf '%s %s\n' "$most_jobs" "$most_load" > "$1.tmp" && mv -f "$1.tmp" "$1"
    sleep 10
  done
}

mkdir -p "$out"
cpu_governor="$(cat /sys/devices/system/cpu/cpu"${cpus%%[-,]*}"/cpufreq/scaling_governor 2>/dev/null || echo unknown)"
jq -n --arg cpu "$(lscpu | sed -n 's/^Model name: *//p' | head -1)" \
  --arg nproc "$(nproc)" --arg kernel "$(uname -srm)" --arg governor "$cpu_governor" \
  --arg boost "$(cat /sys/devices/system/cpu/cpufreq/boost 2>/dev/null || echo unknown)" \
  --arg thp "$(cat /sys/kernel/mm/transparent_hugepage/enabled 2>/dev/null || echo unknown)" \
  --arg mem "$(free -b | awk '/^Mem:/ {print $2}')" --arg cpus "$cpus" \
  --arg normal_fs "$(fstype "$normal_dir")" --arg strict_fs "$(fstype "$strict_dir" 2>/dev/null)" \
  --arg strict_source "$(findmnt -n -o SOURCE -T "$strict_dir" 2>/dev/null || echo unknown)" \
  '{schema: "redline-scoreboard-host-v1", cpu_model: $cpu, logical_cpus: ($nproc | tonumber),
    kernel: $kernel, pinned_cpus: $cpus, governor: $governor, boost: $boost,
    transparent_hugepages: $thp, memory_bytes: ($mem | tonumber),
    normal_dir_fs: $normal_fs, strict_dir_fs: $strict_fs, strict_dir_device: $strict_source}' \
  > "$out/host.json"

IFS=',' read -r -a pair_list <<< "$pairs"
n=${#labels[@]}
for run in $(seq 1 "$runs"); do
  for pair in "${pair_list[@]}"; do
    base="$normal_dir/$bundle"
    [ "$pair" = strict ] && base="$strict_dir/$bundle"
    for step in $(seq 0 $((n - 1))); do
      i=$(( (step + run - 1) % n ))
      label="${labels[$i]}" bin="${binaries[$i]}"
      mkdir -p "$out/$label"
      wait_quiet
      before_jobs="$(runner_jobs)" before_load="$(loadavg)" started="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
      host_sample="$base/host-sample"
      mkdir -p "$base"
      sample_host "$host_sample" &
      sampler=$!
      printf '==> run %s pair %s: %s\n' "$run" "$pair" "$label"
      taskset -c "$cpus" "$bin" run --label "$label" --run "$run" --rows "$rows" --pair "$pair" \
        --reps "$reps" --image-store "$base/images" --work-root "$base/work" \
        --out "$out/$label/run-$run-$pair.jsonl"
      kill "$sampler" 2>/dev/null || true
      wait "$sampler" 2>/dev/null || true
      # No sample to read means the host went unwatched: count it as busy.
      if ! read -r most_jobs most_load < "$host_sample" || [ -z "${most_load:-}" ]; then
        most_jobs=1 most_load="$max_loadavg"
      fi
      jq -n -c --arg label "$label" --argjson run "$run" --arg pair "$pair" --arg started "$started" \
        --arg finished "$(date -u +%Y-%m-%dT%H:%M:%SZ)" --argjson waited_out "$waited_out" \
        --argjson jobs_before "$before_jobs" --argjson jobs_after "$(runner_jobs)" \
        --argjson jobs_max "$most_jobs" --arg load_max "$most_load" \
        --arg load_before "$before_load" --arg load_after "$(loadavg)" --arg max_load "$max_loadavg" \
        '{label: $label, run: $run, pair: $pair, started_utc: $started, finished_utc: $finished,
          waited_out: $waited_out, runner_jobs_before: $jobs_before, runner_jobs_after: $jobs_after,
          runner_jobs_max: $jobs_max, loadavg_before: ($load_before | tonumber),
          loadavg_after: ($load_after | tonumber), loadavg_max: ($load_max | tonumber),
          max_loadavg: ($max_load | tonumber)}' \
        >> "$out/runs.jsonl"
    done
  done
done

for i in "${!labels[@]}"; do
  cp "$(dirname "${binaries[$i]}")/build.json" "$out/${labels[$i]}/build.json"
done
jq -n --arg bundle "$bundle" --argjson labels "$(printf '%s\n' "${labels[@]}" | jq -R . | jq -s -c .)" \
  --argjson rows "$rows" --argjson runs "$runs" --argjson reps "$reps" \
  --argjson pairs "$(printf '%s\n' "${pair_list[@]}" | jq -R . | jq -s -c .)" \
  --arg cpus "$cpus" --arg harness "$harness" --arg sqlite "$sqlite" \
  --arg created "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '{schema: "redline-scoreboard-bundle-v1", bundle: $bundle, labels: $labels, rows: $rows,
    runs: $runs, reps: $reps, pairs: $pairs, pinned_cpus: $cpus, harness_tree: $harness,
    sqlite_source_id: $sqlite, created_utc: $created}' > "$out/bundle.json"

# The measurements are complete: keep the bundle even if summarizing fails.
measured=1
"${binaries[$((n - 1))]}" summarize --bundle "$out"
