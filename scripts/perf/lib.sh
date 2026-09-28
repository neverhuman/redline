#!/usr/bin/env bash
# scripts/perf/lib.sh — shared environment for A/B measurement against
# the redline-testing parity harness.
#
# Sourced by: full.sh, pgo.sh, bolt.sh and any new full-corpus perf scripts.
#
# Conventions match scripts/just/run.sh + scripts/perf/pgo.sh + the
# CI parity gate (ops/ci/lib.sh::ci_resolve_redline_testing_release).
#
# Environment overrides:
#   REDLINE_TESTING_BIN     path to the redline-testing binary (default: the
#                           in-tree runner, target/release/redline-testing,
#                           built by ops/ci/lib.sh ci_install_redline_testing)
#   SQLITE_REF_BIN          path to the sqlite3 reference binary (default: the
#                           pinned build from scripts/sqlite/build-reference.sh)
#   PERF_ROOT               where JSONL outputs land (default: target/perf)
#   PERF_WORKERS            override workers (default 1 for low variance)
#   PERF_TASKSET_CPUS       CPU list passed to taskset (default 2-5)
#   PERF_TASKSET_DISABLE    set non-empty to skip CPU pinning
#   PERF_ORDER              which engine runs first in each sample:
#                           alternate (default), sqlite-first, target-first
#   PERF_KNOWN_FAILURES     known-failures baseline the perf lanes tolerate
#                           (default metadata/sqlite_parity/known-failures.json)
#   PERF_EVIDENCE_BIN       a built perf_evidence binary (default: cargo run)
#   PERF_BUILD_PROFILE, PERF_BUILD_FEATURES, PERF_BUILD_RUSTFLAGS,
#   PERF_PGO_TRAINING_CORPUS
#                           how the measured target was built, as its builder
#                           declares it for build-contract.json; unset means
#                           undeclared (recorded as null), and an empty
#                           PERF_BUILD_RUSTFLAGS means built with no flags
#   CI_REDLINE_TESTING_BIN  CI-resolved binary path (takes precedence)

set -euo pipefail

PERF_ROOT="${PERF_ROOT:-target/perf}"

REDLINE_CORE_ROOT="${REDLINE_CORE_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
# The runner and the reference both live in this checkout; no sibling
# checkout is consulted.
REDLINE_TESTING_BIN_DEFAULT="${REDLINE_CORE_ROOT}/target/release/redline-testing"
REDLINE_TESTING_BIN="${REDLINE_TESTING_BIN:-$REDLINE_TESTING_BIN_DEFAULT}"

SQLITE_REF_BIN_DEFAULT="${SQLITE_REF_BIN_DEFAULT:-${REDLINE_CORE_ROOT}/target/sqlite-reference/3.53.1/bin/sqlite3}"
SQLITE_REF_BIN="${SQLITE_REF_BIN:-$SQLITE_REF_BIN_DEFAULT}"
PERF_KNOWN_FAILURES="${PERF_KNOWN_FAILURES:-${REDLINE_CORE_ROOT}/metadata/sqlite_parity/known-failures.json}"

# CI overrides everything (ops/ci/lib.sh::ci_resolve_redline_testing_release
# sets CI_REDLINE_TESTING_BIN after SHA-256-verifying a pinned release).
if [ -n "${CI_REDLINE_TESTING_BIN:-}" ]; then
  REDLINE_TESTING_BIN="$CI_REDLINE_TESTING_BIN"
fi

perf_evidence() {
  if [ -n "${PERF_EVIDENCE_BIN:-}" ]; then
    "$PERF_EVIDENCE_BIN" "$@"
    return
  fi
  cargo run --quiet --locked --manifest-path "$REDLINE_CORE_ROOT/Cargo.toml" \
    -p redlinedb-bench --bin perf_evidence -- "$@"
}

perf_require_bins() {
  local target_bin="$1"
  if [ ! -x "$target_bin" ]; then
    printf 'perf: target binary not executable: %s\n' "$target_bin" >&2
    exit 2
  fi
  if [ ! -x "$REDLINE_TESTING_BIN" ]; then
    printf 'perf: redline-testing missing: %s\n' "$REDLINE_TESTING_BIN" >&2
    printf '       build it with: bash -c %s\n' "'. ops/ci/lib.sh && ci_install_redline_testing'" >&2
    exit 2
  fi
  if [ ! -x "$SQLITE_REF_BIN" ]; then
    printf 'perf: sqlite3 reference missing: %s\n' "$SQLITE_REF_BIN" >&2
    printf '       set SQLITE_REF_BIN or run scripts/sqlite/build-reference.sh\n' >&2
    exit 2
  fi
  # Refuse to time sqlite3 vs itself — guards against accidental misuse.
  perf_evidence assert-distinct-binaries "$target_bin" "$SQLITE_REF_BIN"
}

perf_tmp_root() {
  local tag="${1:-default}"
  if [ -d /dev/shm ] && [ -w /dev/shm ]; then
    printf '/dev/shm/redline-testing-perf-%s\n' "$tag"
  else
    printf '%s/redline-testing-perf-%s\n' "${TMPDIR:-/tmp}" "$tag"
  fi
}

perf_quiet_system() {
  # Best-effort variance reduction; never fail if we lack permission.
  if [ -w /proc/sys/vm/drop_caches ]; then
    sync
    printf 3 > /proc/sys/vm/drop_caches 2>/dev/null || true
  fi
  if command -v cpupower >/dev/null 2>&1 && [ "$(id -u)" = 0 ]; then
    cpupower frequency-set -g performance >/dev/null 2>&1 || true
  fi
}

# Run the complete parity workload through the verified external runner with
# variance-controlled defaults.
#
# Usage: perf_run_jsonl <target-bin> <reps> <warmup> <output.jsonl> <tmp-tag>
perf_run_jsonl() {
  local target_bin="$1" reps="$2" warmup="$3" out="$4" tag="$5"
  local tmp
  tmp="$(perf_tmp_root "$tag")"
  mkdir -p "$(dirname "$out")" "$tmp"
  perf_quiet_system

  local taskset_cmd=()
  if [ -z "${PERF_TASKSET_DISABLE:-}" ] && command -v taskset >/dev/null 2>&1; then
    taskset_cmd=("taskset" "-c" "${PERF_TASKSET_CPUS:-2-5}")
  fi

  # The official lane's durability, with its notice off: the notice is
  # stderr output of every target run, and the runner compares stderr.
  REDLINEDB_DEFAULT_DURABILITY=normal \
  REDLINEDB_QUIET_DURABILITY=1 \
  "${taskset_cmd[@]}" \
    "$REDLINE_TESTING_BIN" run \
      --target-bin   "$target_bin" \
      --sqlite-bin   "$SQLITE_REF_BIN" \
      --suite        sqlite_parity \
      --workers      "${PERF_WORKERS:-1}" \
      --tmp-root     "$tmp" \
      --repetitions  "$reps" \
      --warmup       "$warmup" \
      --order        "${PERF_ORDER:-alternate}" \
      --output       "$out"
}

# The runner command of a PGO or BOLT training run: every sqlite_parity
# case once, no warmup, under the official lane's quiet normal durability,
# and held to the known-failures baseline as the official lane is. The
# baseline lists the cases the current target fails, so without it every
# training run of a healthy build fails; with it, only a failure the
# baseline does not list (or a listed case that now passes) fails the run.
#
# Usage: perf_training_command <target-bin> <tmp-root> <output.jsonl>
#   sets the array PERF_TRAINING_COMMAND
perf_training_command() {
  local target_bin="$1" tmp_root="$2" out="$3"
  # shellcheck disable=SC2034 # read by the scripts that source this file
  PERF_TRAINING_COMMAND=(
    env REDLINEDB_DEFAULT_DURABILITY=normal REDLINEDB_QUIET_DURABILITY=1
    "$REDLINE_TESTING_BIN" run
    --target-bin "$target_bin"
    --sqlite-bin "$SQLITE_REF_BIN"
    --suite sqlite_parity
    --sqlite-known-failures "$PERF_KNOWN_FAILURES"
    --workers "${PERF_WORKERS:-10}"
    --tmp-root "$tmp_root"
    --repetitions 1
    --warmup 0
    --output "$out"
  )
}

# Write build-contract.json for a measured target (BM3-05): rustc -vV, the
# target, reference and runner digests and versions, the reference's
# PRAGMA compile_options, and the target's build as its builder declared it
# through the PERF_BUILD_* variables above. Nothing undeclared is guessed.
#
# Usage: perf_build_contract <output.json> <target-bin>
perf_build_contract() {
  local output="$1" target_bin="$2"
  local -a declared=()
  if [ -n "${PERF_BUILD_PROFILE+set}" ]; then
    declared+=(--profile "$PERF_BUILD_PROFILE")
  fi
  if [ -n "${PERF_BUILD_FEATURES+set}" ]; then
    declared+=("--features=$PERF_BUILD_FEATURES")
  fi
  if [ -n "${PERF_BUILD_RUSTFLAGS+set}" ]; then
    declared+=("--rustflags=$PERF_BUILD_RUSTFLAGS")
  fi
  if [ -n "${PERF_PGO_TRAINING_CORPUS:-}" ]; then
    declared+=(--pgo-training-corpus "$PERF_PGO_TRAINING_CORPUS")
  fi
  perf_evidence build-contract \
    --output "$output" \
    --target-bin "$target_bin" \
    --reference-bin "$SQLITE_REF_BIN" \
    --runner-bin "$REDLINE_TESTING_BIN" \
    "${declared[@]}"
}

# Print the case-level summary of a JSONL file (perf_evidence
# summarize-jsonl; extra arguments such as --expected-repetitions pass
# through). Used by the runner scripts so the user sees results inline.
perf_summarize_jsonl() {
  perf_evidence summarize-jsonl "$@"
}

# Refuse a target that is not a release measurement (L-05): a failpoints
# build (it carries the kernel's failpoint names) or a build with debug
# assertions or overflow checks (it carries the standard library's
# precondition and overflow panic messages, which a release build compiles
# out). Exits 2 naming the marker it found.
#
# Usage: perf_check_release_binary <label> <binary>
perf_check_release_binary() {
  local label="$1" binary="$2" marker
  if [ ! -f "$binary" ] || [ ! -x "$binary" ]; then
    printf 'perf: %s: %s is not an executable file\n' "$label" "$binary" >&2
    exit 2
  fi
  for marker in 'engine::commit::before_publish' 'wal::write_encoded'; do
    if grep -a -q -F -- "$marker" "$binary"; then
      printf 'perf: %s: %s contains the failpoint name %s: a failpoints build is not a release measurement\n' \
        "$label" "$binary" "$marker" >&2
      exit 2
    fi
  done
  for marker in 'unsafe precondition(s) violated' 'attempt to add with overflow'; do
    if grep -a -q -F -- "$marker" "$binary"; then
      printf 'perf: %s: %s contains "%s": it was built with debug assertions or overflow checks, not as a release build\n' \
        "$label" "$binary" "$marker" >&2
      exit 2
    fi
  done
}
