#!/usr/bin/env bash
# Named release bench bundle (L-05). Times N labelled RedlineDB binaries
# against the pinned SQLite reference with one runner, K runs per binary,
# interleaved across the binaries, and writes
#   benchmark-results/sqlite-parity/releases/<bundle>/
#     bundle.json                  what ran: protocol, labels, runs
#     host.json                    host facts and each run's load
#     cases.json                   the runner's corpus listing (id, name, ...)
#     case-list.txt                the --case-list, when one narrowed the corpus
#     cohorts/medium-set.txt       the medium cohort sliced in the summary
#     <label>/build-contract.json  perf_evidence build-contract for the binary
#     <label>/build.json           scripts/perf/build-version.sh's record, if any
#     <label>/run-<k>/raw.jsonl    the runner's records, completion marker,
#                                  evidence (manifest, provenance, ...) and log
#     summary.json                 perf_evidence summarize-bundle
#     README.md                    how the bundle was measured and what it means
#
# It builds nothing. Build old versions with scripts/perf/build-version.sh,
# which leaves a build.json beside each binary; this script declares that
# build in the contract. A binary without one is recorded as undeclared, and
# its summary is not publishable.
#
# Usage:
#   scripts/perf/release-bench.sh --bundle <name> [options] <label>=<binary> ...
# Labels go oldest first: each label's change is measured against the one
# before it.
#
# Options:
#   --runs K             runs per label (default 3). A publishable bundle
#                        needs K >= 3.
#   --cpus LIST          taskset CPU list for every run (default 2-5); `none`
#                        runs unpinned (diagnostic, not publishable)
#   --max-loadavg X      reject a run whose 1-minute load average before or
#                        after it exceeds X (default: nproc / 4)
#   --durability MODE    normal (default): REDLINEDB_DEFAULT_DURABILITY=normal
#                        and REDLINEDB_QUIET_DURABILITY=1, as the official
#                        lane runs; default: neither, so each binary runs its
#                        built-in default. v4.0.9 and older have no such knob
#                        and always run Strict, so compare them with
#                        --durability default.
#   --case-list FILE     run only these cases (diagnostic, not publishable)
#   --medium-cohort FILE the cohort sliced in the summary (default
#                        bench/perf/cases/medium-set.txt, which must match
#                        its .sha256); --no-medium-cohort for none
#   --out-root DIR       where bundles go
#                        (default benchmark-results/sqlite-parity/releases)
#
# Each run is `redline-testing run --suite sqlite_parity --workers 1
# --repetitions 3 --warmup 1 --order alternate` with a fresh temp root on
# tmpfs (/dev/shm), pinned with taskset. It is accepted only when:
#   - the 1-minute load average before and after it is within the threshold;
#   - perf_evidence validate-run finds the complete requested experiment;
#   - a non-zero runner exit is explained by at least one failed case.
# A failed case is data (an older version fails more of today's corpus), not
# an error. Anything else stops the bundle, which keeps its directory
# without a bundle.json, so it cannot be summarized.
#
# Environment: REDLINE_TESTING_BIN, SQLITE_REF_BIN, PERF_EVIDENCE_BIN (see
# scripts/perf/lib.sh); RELEASE_BENCH_TMPFS (default /dev/shm);
# RELEASE_BENCH_LOADAVG (default /proc/loadavg);
# RELEASE_BENCH_RUNNER_UNITS (default 'actions.runner.* jope-runner@*');
# RELEASE_BENCH_JOB_PATTERN (default Runner.Worker).
#
# Exit status: 2 for a usage or precondition error, 3 for a rejected run,
# otherwise perf_evidence summarize-bundle's.

set -euo pipefail

invocation_dir="$PWD"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(git -C "$script_dir" rev-parse --show-toplevel)"

die() {
  printf 'release-bench: %s\n' "$*" >&2
  exit 2
}

absolute() {
  case "$1" in
    /*) printf '%s\n' "$1" ;;
    *) printf '%s/%s\n' "$invocation_dir" "$1" ;;
  esac
}

bundle='' runs=3 cpus='2-5' max_loadavg='' durability=normal case_list=''
medium_cohort="$repo_root/bench/perf/cases/medium-set.txt"
out_root="$repo_root/benchmark-results/sqlite-parity/releases"
labels=() binaries=()
while [ $# -gt 0 ]; do
  case "$1" in
    --bundle) bundle="${2:?--bundle needs a name}"; shift 2 ;;
    --runs) runs="${2:?--runs needs a count}"; shift 2 ;;
    --cpus) cpus="${2:?--cpus needs a list}"; shift 2 ;;
    --max-loadavg) max_loadavg="${2:?--max-loadavg needs a number}"; shift 2 ;;
    --durability) durability="${2:?--durability needs a mode}"; shift 2 ;;
    --case-list) case_list="$(absolute "${2:?--case-list needs a file}")"; shift 2 ;;
    --medium-cohort) medium_cohort="$(absolute "${2:?--medium-cohort needs a file}")"; shift 2 ;;
    --no-medium-cohort) medium_cohort=''; shift ;;
    --out-root) out_root="$(absolute "${2:?--out-root needs a directory}")"; shift 2 ;;
    -h|--help) sed -n '2,/^$/s/^# \{0,1\}//p' "${BASH_SOURCE[0]}"; exit 0 ;;
    --*) die "unknown option $1" ;;
    *=*) labels+=("${1%%=*}"); binaries+=("$(absolute "${1#*=}")"); shift ;;
    *) die "expected <label>=<binary>, got $1" ;;
  esac
done

cd "$repo_root"
source "$script_dir/lib.sh"

name_re='^[A-Za-z0-9][A-Za-z0-9._+-]*$'
[[ "$bundle" =~ $name_re ]] || die "--bundle must match $name_re, got '$bundle'"
[[ "$runs" =~ ^[1-9][0-9]*$ ]] || die "--runs must be a positive integer, got '$runs'"
[ "${#labels[@]}" -gt 0 ] || die "no <label>=<binary> given"
case "$durability" in normal|default) ;; *) die "--durability is normal or default, got '$durability'" ;; esac
if [ "$cpus" = none ]; then
  pin=()
  cpus_json=null
else
  [[ "$cpus" =~ ^[0-9]+(-[0-9]+)?(,[0-9]+(-[0-9]+)?)*$ ]] || die "--cpus must be a CPU list like 2-5 or none, got '$cpus'"
  command -v taskset >/dev/null || die "taskset is missing; install util-linux or pass --cpus none"
  pin=(taskset -c "$cpus")
  cpus_json="$(jq -n --arg cpus "$cpus" '$cpus')"
fi
nproc_count="$(nproc)"
if [ -z "$max_loadavg" ]; then
  max_loadavg="$(( nproc_count / 4 > 0 ? nproc_count / 4 : 1 ))"
fi
if ! [[ "$max_loadavg" =~ ^[0-9]+(\.[0-9]+)?$ ]] || ! awk -v x="$max_loadavg" 'BEGIN { exit !(x > 0) }'; then
  die "--max-loadavg must be a positive number, got '$max_loadavg'"
fi
declare -A seen=()
for label in "${labels[@]}"; do
  [[ "$label" =~ $name_re ]] || die "label must match $name_re, got '$label'"
  [ -z "${seen[$label]:-}" ] || die "label $label is given twice"
  seen[$label]=1
done
tmpfs_base="${RELEASE_BENCH_TMPFS:-/dev/shm}"
if [ ! -d "$tmpfs_base" ] || [ ! -w "$tmpfs_base" ]; then
  die "temp root base $tmpfs_base is not a writable directory"
fi
tmp_filesystem="$(stat -f -c %T "$tmpfs_base")"
if [ "$tmp_filesystem" != tmpfs ]; then
  printf 'release-bench: warning: %s is on %s, not tmpfs; the summary will not be publishable\n' \
    "$tmpfs_base" "$tmp_filesystem" >&2
fi
loadavg_file="${RELEASE_BENCH_LOADAVG:-/proc/loadavg}"
[ -r "$loadavg_file" ] || die "cannot read the load average from $loadavg_file"
command -v jq >/dev/null || die "jq is missing"

# ---- binaries -------------------------------------------------------------
declare -A label_json=() build_record=() durability_env=()
for i in "${!labels[@]}"; do
  label="${labels[$i]}" binary="${binaries[$i]}"
  perf_check_release_binary "$label" "$binary"
  perf_require_bins "$binary"
  binary="$(realpath "$binary")"
  binaries[i]="$binary"
  sha="$(sha256sum "$binary" | awk '{print $1}')"
  if grep -a -q -F REDLINEDB_DEFAULT_DURABILITY "$binary"; then
    durability_env[$label]=true
  else
    durability_env[$label]=false
    if [ "$durability" = normal ]; then
      printf 'release-bench: warning: %s has no REDLINEDB_DEFAULT_DURABILITY knob and will run its built-in default (Strict); the summary will not be publishable. Use --durability default to compare it.\n' "$label" >&2
    fi
  fi
  record="$(dirname "$binary")/build.json"
  source_ref=null source_commit=null
  if [ -f "$record" ]; then
    jq -e --arg sha "$sha" \
      '.schema_version == "redline-version-build-v1" and .binary_sha256 == $sha' "$record" >/dev/null \
      || die "$label: $record does not describe $binary (schema redline-version-build-v1, sha256 $sha)"
    build_record[$label]="$record"
    source_ref="$(jq '.source_ref' "$record")"
    source_commit="$(jq '.source_commit' "$record")"
  fi
  case "$binary" in
    "$repo_root"/*) shown="${binary#"$repo_root"/}" ;;
    *) shown="$binary" ;;
  esac
  label_json[$label]="$(jq -n --arg label "$label" --arg binary "$shown" --arg sha "$sha" \
    --argjson source_ref "$source_ref" --argjson source_commit "$source_commit" \
    --arg contract "$label/build-contract.json" \
    --argjson record "$([ -n "${build_record[$label]:-}" ] && jq -n --arg p "$label/build.json" '$p' || echo null)" \
    --argjson durability_env "${durability_env[$label]}" \
    '{label: $label, binary: $binary, binary_sha256: $sha, source_ref: $source_ref,
      source_commit: $source_commit, build_contract: $contract, build_record: $record,
      durability_env: $durability_env}')"
done

# ---- corpus, case list and cohort, checked before anything is written -------
bundle_dir="$out_root/$bundle"
[ ! -e "$bundle_dir" ] || die "$bundle_dir already exists; pick another --bundle name"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
env -u REDLINE_TESTING_PINNED_ONLY "$REDLINE_TESTING_BIN" list --suite sqlite_parity --format json \
  > "$work/listing.json"
# missing_ids <json array of numeric ids>: those the corpus listing lacks.
missing_ids() {
  jq -r --argjson ids "$1" '[.[].id] as $have | $ids - $have | map(tostring) | join(" ")' \
    "$work/listing.json"
}
case_args=()
case_list_json=null
if [ -n "$case_list" ]; then
  [ -f "$case_list" ] || die "--case-list $case_list is not a file"
  case_ids_text="$(perf_evidence case-list-ids "$case_list")"
  mapfile -t case_ids <<< "$case_ids_text"
  ids_json="$(jq -R 'tonumber' <<< "$case_ids_text" | jq -s .)"
  missing="$(missing_ids "$ids_json")"
  [ -z "$missing" ] || die "--case-list names cases the corpus does not have: $missing"
  jq --argjson ids "$ids_json" \
    '[.[] | select(.id as $id | $ids | index($id)) | {id, name, category, priority, profile}]' \
    "$work/listing.json" > "$work/cases.json"
  for id in "${case_ids[@]}"; do case_args+=(--case-id "$id"); done
  case_list_json="$(jq -n --arg sha "$(sha256sum "$case_list" | awk '{print $1}')" \
    --arg source "$case_list" '{path: "case-list.txt", sha256: $sha, source: $source}')"
else
  jq '[.[] | {id, name, category, priority, profile}]' "$work/listing.json" > "$work/cases.json"
fi
expected_cases="$(jq 'length' "$work/cases.json")"

medium_json=null
if [ -n "$medium_cohort" ]; then
  [ -f "$medium_cohort" ] || die "medium cohort $medium_cohort is not a file"
  if [ -f "$medium_cohort.sha256" ]; then
    (cd "$(dirname "$medium_cohort")" && sha256sum --quiet -c "$(basename "$medium_cohort").sha256") \
      || die "medium cohort $medium_cohort does not match its .sha256"
  elif [ "$medium_cohort" = "$repo_root/bench/perf/cases/medium-set.txt" ]; then
    die "$medium_cohort.sha256 is missing"
  fi
  cohort_ids="$(perf_evidence case-list-ids "$medium_cohort" | jq -R 'tonumber' | jq -s .)"
  missing="$(missing_ids "$cohort_ids")"
  [ -z "$missing" ] || die "medium cohort $medium_cohort names cases the corpus does not have: $missing"
  case "$medium_cohort" in
    "$repo_root"/*) medium_source="${medium_cohort#"$repo_root"/}" ;;
    *) medium_source="$medium_cohort" ;;
  esac
  medium_json="$(jq -n --arg sha "$(sha256sum "$medium_cohort" | awk '{print $1}')" \
    --arg source "$medium_source" '{path: "cohorts/medium-set.txt", sha256: $sha, source: $source}')"
fi

# ---- the bundle directory ---------------------------------------------------
mkdir -p "$out_root"
# No -p: an existing directory is another bundle's, never this one's.
mkdir "$bundle_dir" || die "could not create $bundle_dir"
started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
printf '==> release bench %s: %d label(s) x %d run(s) -> %s\n' "$bundle" "${#labels[@]}" "$runs" "$bundle_dir"
cp "$work/cases.json" "$bundle_dir/cases.json"
if [ -n "$case_list" ]; then cp "$case_list" "$bundle_dir/case-list.txt"; fi
if [ -n "$medium_cohort" ]; then
  mkdir "$bundle_dir/cohorts"
  cp "$medium_cohort" "$bundle_dir/cohorts/medium-set.txt"
fi

for i in "${!labels[@]}"; do
  label="${labels[$i]}"
  mkdir "$bundle_dir/$label"
  (
    unset PERF_BUILD_PROFILE PERF_BUILD_FEATURES PERF_BUILD_RUSTFLAGS PERF_PGO_TRAINING_CORPUS
    if [ -n "${build_record[$label]:-}" ]; then
      record="${build_record[$label]}"
      cp "$record" "$bundle_dir/$label/build.json"
      PERF_BUILD_PROFILE="$(jq -r '.profile' "$record")"
      PERF_BUILD_FEATURES="$(jq -r '.features' "$record")"
      PERF_BUILD_RUSTFLAGS="$(jq -r '.rustflags' "$record")"
      export PERF_BUILD_PROFILE PERF_BUILD_FEATURES PERF_BUILD_RUSTFLAGS
    fi
    perf_build_contract "$bundle_dir/$label/build-contract.json" "${binaries[$i]}" > /dev/null
  )
done

# ---- host facts -------------------------------------------------------------
cpu_list() {
  local part start end cpu
  IFS=',' read -ra parts <<< "$1"
  for part in "${parts[@]}"; do
    start="${part%-*}" end="${part#*-}"
    for ((cpu = start; cpu <= end; cpu++)); do printf '%s\n' "$cpu"; done
  done
}
governors_json='{}'
if [ "$cpus" != none ]; then
  while read -r cpu; do
    file="/sys/devices/system/cpu/cpu$cpu/cpufreq/scaling_governor"
    governor="$(cat "$file" 2>/dev/null || echo unavailable)"
    governors_json="$(jq -c --arg cpu "cpu$cpu" --arg governor "$governor" '. + {($cpu): $governor}' <<< "$governors_json")"
  done < <(cpu_list "$cpus")
fi
units_json='[]'
if command -v systemctl >/dev/null; then
  read -ra unit_patterns <<< "${RELEASE_BENCH_RUNNER_UNITS:-actions.runner.* jope-runner@*}"
  units_json="$(systemctl list-units --type=service --state=active --no-legend --plain \
    "${unit_patterns[@]}" 2>/dev/null | awk '{print $1}' | jq -R . | jq -s -c . || echo '[]')"
fi
cpu_model="$(awk -F': ' '/^model name/ {print $2; exit}' /proc/cpuinfo 2>/dev/null || true)"
host_static="$(jq -n --arg hostname "$(hostname)" --arg kernel "$(uname -srm)" \
  --arg cpu_model "${cpu_model:-unknown}" --argjson nproc "$nproc_count" \
  --argjson pinned "$cpus_json" --arg tmp_filesystem "$tmp_filesystem" \
  --argjson governors "$governors_json" --argjson units "$units_json" --argjson max "$max_loadavg" \
  '{schema_version: "redline-release-bench-host-v1", hostname: $hostname, kernel: $kernel,
    cpu_model: $cpu_model, nproc: $nproc, pinned_cpus: $pinned, tmp_filesystem: $tmp_filesystem,
    governors: $governors, runner_units_active: $units, max_loadavg: $max}')"
host_runs=()
write_host() {
  printf '%s\n' "${host_runs[@]}" | jq -s --argjson host "$host_static" '$host + {runs: .}' \
    > "$bundle_dir/host.json"
}
load1() { awk '{print $1}' "$loadavg_file"; }
loads() { awk '{printf "[%s,%s,%s]", $1, $2, $3}' "$loadavg_file"; }
runner_jobs() { pgrep -c -f "${RELEASE_BENCH_JOB_PATTERN:-Runner.Worker}" 2>/dev/null || true; }
above() { awk -v a="$1" -v b="$max_loadavg" 'BEGIN { exit !(a > b) }'; }

if [ "$durability" = normal ]; then
  durability_env_args=(REDLINEDB_DEFAULT_DURABILITY=normal REDLINEDB_QUIET_DURABILITY=1)
else
  durability_env_args=(-u REDLINEDB_DEFAULT_DURABILITY -u REDLINEDB_QUIET_DURABILITY)
fi
run_command="redline-testing run --suite sqlite_parity --target-bin <binary> --sqlite-bin <reference> --workers 1 --repetitions 3 --warmup 1 --order alternate --tmp-root $tmpfs_base/rl-bench-<bundle>-<pid>-<sequence> --progress never --output <label>/run-<k>/raw.jsonl"
if [ "${#case_args[@]}" -gt 0 ]; then run_command+=" --case-id <each case-list id>"; fi
if [ "$cpus" != none ]; then run_command="taskset -c $cpus $run_command"; fi
if [ "$durability" = normal ]; then
  run_command="REDLINEDB_DEFAULT_DURABILITY=normal REDLINEDB_QUIET_DURABILITY=1 $run_command"
fi

# ---- runs ---------------------------------------------------------------------
# reject <label> <k> <sequence> <started> <before> <jobs before> <exit> <reason>
reject() {
  host_runs+=("$(jq -n --argjson sequence "$3" --arg label "$1" --argjson run "$2" \
    --arg started "$4" --arg finished "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --argjson before "$5" --argjson after "$(loads)" --argjson jobs_before "$6" \
    --argjson jobs_after "$(runner_jobs)" --argjson exit "$7" --arg reason "$8" \
    '{sequence: $sequence, label: $label, run: $run, started_at_utc: $started,
      finished_at_utc: $finished, loadavg_before: $before, loadavg_after: $after,
      runner_jobs_before: $jobs_before, runner_jobs_after: $jobs_after,
      runner_exit: $exit, accepted: false, reason: $reason}')")
  write_host
  printf 'release-bench: %s run %s rejected: %s\n' "$1" "$2" "$8" >&2
  printf 'release-bench: bundle directory kept for diagnosis, without bundle.json: %s\n' "$bundle_dir" >&2
  exit 3
}

run_entries=()
sequence=0
count="${#labels[@]}"
for ((k = 1; k <= runs; k++)); do
  for ((slot = 0; slot < count; slot++)); do
    # Rotate the starting label each round so no label always runs first.
    index=$(( (slot + k - 1) % count ))
    label="${labels[$index]}" binary="${binaries[$index]}"
    sequence=$((sequence + 1))
    run_dir="$bundle_dir/$label/run-$k"
    raw="$run_dir/raw.jsonl"
    mkdir "$run_dir"
    started="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    before="$(loads)"
    jobs_before="$(runner_jobs)"
    if above "$(load1)"; then
      reject "$label" "$k" "$sequence" "$started" "$before" "$jobs_before" 0 \
        "1-minute load average $(load1) before the run exceeds $max_loadavg"
    fi
    tmp_root="$tmpfs_base/rl-bench-$bundle-$$-$sequence"
    mkdir "$tmp_root"
    printf '  [%d] %s run %d/%d ...\n' "$sequence" "$label" "$k" "$runs"
    rc=0
    env -u REDLINE_TESTING_PINNED_ONLY "${durability_env_args[@]}" "${pin[@]}" \
      "$REDLINE_TESTING_BIN" run \
        --suite sqlite_parity \
        --target-bin "$binary" \
        --sqlite-bin "$SQLITE_REF_BIN" \
        --workers 1 \
        --repetitions 3 \
        --warmup 1 \
        --order alternate \
        --tmp-root "$tmp_root" \
        --progress never \
        --output "$raw" \
        "${case_args[@]}" > "$run_dir/runner.log" 2>&1 || rc=$?
    chmod -R u+rwX "$tmp_root" 2>/dev/null || true
    rm -rf "$tmp_root" 2>/dev/null || printf 'release-bench: could not remove %s\n' "$tmp_root" >&2
    if ! perf_evidence validate-run --expected-cases "$expected_cases" \
      --case-manifest "$bundle_dir/cases.json" --reps 3 --warmup 1 "$raw" \
      > "$run_dir/validation.txt" 2>&1; then
      reject "$label" "$k" "$sequence" "$started" "$before" "$jobs_before" "$rc" \
        "not the complete requested run (runner exit $rc): $(tail -n 1 "$run_dir/validation.txt")"
    fi
    failed="$(jq -r 'select(.status == "failed") | .case_id' "$raw" | sort -u | wc -l | tr -d ' ')"
    if [ "$rc" -ne 0 ] && [ "$failed" -eq 0 ]; then
      reject "$label" "$k" "$sequence" "$started" "$before" "$jobs_before" "$rc" \
        "the runner exited $rc but no case failed"
    fi
    if above "$(load1)"; then
      reject "$label" "$k" "$sequence" "$started" "$before" "$jobs_before" "$rc" \
        "1-minute load average $(load1) after the run exceeds $max_loadavg"
    fi
    host_runs+=("$(jq -n --argjson sequence "$sequence" --arg label "$label" --argjson run "$k" \
      --arg started "$started" --arg finished "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
      --argjson before "$before" --argjson after "$(loads)" --argjson jobs_before "$jobs_before" \
      --argjson jobs_after "$(runner_jobs)" --argjson exit "$rc" \
      '{sequence: $sequence, label: $label, run: $run, started_at_utc: $started,
        finished_at_utc: $finished, loadavg_before: $before, loadavg_after: $after,
        runner_jobs_before: $jobs_before, runner_jobs_after: $jobs_after,
        runner_exit: $exit, accepted: true, reason: null}')")
    write_host
    run_entries+=("$(jq -n --argjson sequence "$sequence" --arg label "$label" --argjson run "$k" \
      --arg raw "$label/run-$k/raw.jsonl" '{sequence: $sequence, label: $label, run: $run, raw: $raw}')")
    printf '      %s (%s failed case(s), runner exit %d)\n' "$(head -n 1 "$run_dir/validation.txt" | cut -c1-120)" "$failed" "$rc"
  done
done
finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# ---- bundle.json, README.md, summary.json -------------------------------------
protocol_json="$(jq -n --argjson cpus "$cpus_json" --arg durability "$durability" \
  --arg tmp_root "$tmpfs_base/rl-bench-<bundle>-<pid>-<sequence>" \
  --argjson max "$max_loadavg" --arg command "$run_command" \
  '{suite: "sqlite_parity", workers: 1, repetitions: 3, warmup: 1, order: "alternate",
    cpus: $cpus, tmp_root: $tmp_root, durability: $durability,
    measurement_boundary: "cli_case_wall_time", max_loadavg: $max, command: $command}')"
labels_array="$(for label in "${labels[@]}"; do printf '%s\n' "${label_json[$label]}"; done | jq -s .)"
jq -n --arg bundle "$bundle" --arg started "$started_at" --arg finished "$finished_at" \
  --argjson runs "$runs" --argjson protocol "$protocol_json" \
  --argjson count "$expected_cases" --argjson case_list "$case_list_json" \
  --argjson medium "$medium_json" --argjson labels "$labels_array" \
  --argjson run_entries "$(printf '%s\n' "${run_entries[@]}" | jq -s .)" \
  '{schema_version: "redline-release-bench-bundle-v1", bundle: $bundle,
    started_at_utc: $started, finished_at_utc: $finished, runs_per_label: $runs,
    protocol: $protocol,
    cases: {manifest: "cases.json", count: $count, case_list: $case_list},
    medium_cohort: $medium, labels: $labels, runs: $run_entries}' > "$bundle_dir/bundle.json"

bash "$script_dir/release-bench-readme.sh" "$bundle_dir" > "$bundle_dir/README.md"

perf_evidence summarize-bundle --bundle "$bundle_dir"
printf 'wrote %s\n' "$bundle_dir"
printf 'next: redline-testing version-history --bundle %s [--readme README.md]\n' "${bundle_dir#"$repo_root"/}"
