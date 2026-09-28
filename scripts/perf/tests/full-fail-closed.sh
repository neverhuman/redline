#!/usr/bin/env bash
# Fail-closed fixtures for scripts/perf/full.sh (BM3-01).
#
# Each scenario runs the real full.sh against the stub runner
# (stub-redline-testing.sh), a stub sqlite3 and a stub target, in its own
# PERF_ROOT, and checks the exit status and the artifacts. A perf run may
# succeed only when the requested experiment completed: every listed case,
# one warmup and measured:1..3 each, the runner's completion marker, and no
# failure beyond the known-failures baseline.
#
# Usage: scripts/perf/tests/full-fail-closed.sh
# Needs bash, jq and sha256sum. PERF_EVIDENCE_BIN may name a built
# perf_evidence binary; otherwise full.sh runs it through cargo.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
fixtures_dir="$repo_root/scripts/perf/tests"
work="$(mktemp -d "${TMPDIR:-/tmp}/perf-full-fixtures.XXXXXX")"
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/bin"
cp "$fixtures_dir/stub-redline-testing.sh" "$work/bin/redline-testing"
cat > "$work/bin/sqlite3" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
  --version) echo "3.53.1 2026-01-01 00:00:00 stub (64-bit)" ;;
  *) printf 'COMPILER=stub\nENABLE_FTS5\nTHREADSAFE=1\n' ;;
esac
EOF
cat > "$work/bin/redlinedb" <<'EOF'
#!/usr/bin/env bash
echo "redlinedb 0.0.0-perf-fixture"
EOF
chmod +x "$work/bin/redline-testing" "$work/bin/sqlite3" "$work/bin/redlinedb"

# The fixture baseline tolerates case 10002 failing as a target semantic
# failure, and nothing else.
cat > "$work/known-failures.json" <<'EOF'
{
  "schema_version": "redline-testing-sqlite-known-failures-v1",
  "description": "perf fail-closed fixture",
  "failures": [
    {"suite": "sqlite_parity", "case_id": "10002", "name": "CASE_10002",
     "stage": "target_semantic_failure", "reason": "fixture", "owner": "fixture"}
  ]
}
EOF

export REDLINE_TESTING_BIN="$work/bin/redline-testing"
export SQLITE_REF_BIN="$work/bin/sqlite3"
export PERF_KNOWN_FAILURES="$work/known-failures.json"
export PERF_TASKSET_DISABLE=1
unset CI_REDLINE_TESTING_BIN PERF_WORKERS PERF_ORDER PERF_RUN_DIR_OUT || true
target="$work/bin/redlinedb"

failures=0
report() {
  local verdict="$1" name="$2" detail="$3"
  printf '%-4s %-32s %s\n' "$verdict" "$name" "$detail"
  if [ "$verdict" != ok ]; then
    failures=$((failures + 1))
    if [ -f "$work/$name.log" ]; then
      sed 's/^/       | /' "$work/$name.log"
    fi
  fi
}

# run_full <name> <mode>: sets rc and root.
run_full() {
  local name="$1" mode="$2"
  root="$work/root-$name"
  mkdir -p "$root"
  rc=0
  STUB_MODE="$mode" STUB_ARGS_LOG="$work/$name.args" PERF_ROOT="$root" \
    bash "$repo_root/scripts/perf/full.sh" "$target" "$name" > "$work/$name.log" 2>&1 || rc=$?
}

# expect_failure <name> <mode> [expected exit]
expect_failure() {
  local name="$1" mode="$2" want="${3:-}"
  run_full "$name" "$mode"
  if [ "$rc" -eq 0 ]; then
    report FAIL "$name" "full.sh exited 0; the run must be rejected"
  elif [ -n "$want" ] && [ "$rc" -ne "$want" ]; then
    report FAIL "$name" "full.sh exited $rc; expected $want"
  elif grep -q '^== summary ==' "$work/$name.log"; then
    report FAIL "$name" "full.sh printed a summary for a rejected run"
  else
    report ok "$name" "rejected (exit $rc)"
  fi
}

# Probes from the shard-03 audit, plus the missing pieces of a run.
expect_failure interrupted-child interrupted 137
expect_failure empty-output-exit-0 empty
expect_failure braces-output braces
expect_failure truncated-jsonl truncated
expect_failure missing-repetition missing-repetition
expect_failure duplicate-sample duplicate-sample
expect_failure missing-case missing-case
expect_failure tampered-marker tampered-marker
expect_failure nonzero-without-failure nonzero-without-failure 1
STUB_FAIL_CASE=10000 expect_failure unexpected-failure unexpected-failure

# A stale all.jsonl (and a stale shared alias) that record a pass for the
# case the current run fails must not stand in for the current run.
name=stale-all-jsonl
root="$work/root-$name"
mkdir -p "$root"
STUB_MODE=complete "$REDLINE_TESTING_BIN" run --repetitions 3 --warmup 1 --output "$root/all.jsonl"
cp "$root/all.jsonl" "$root/sqlite_parity.raw.jsonl"
rc=0
STUB_MODE=unexpected-failure STUB_FAIL_CASE=10000 PERF_ROOT="$root" \
  bash "$repo_root/scripts/perf/full.sh" "$target" "$name" > "$work/$name.log" 2>&1 || rc=$?
if [ "$rc" -eq 0 ]; then
  report FAIL "$name" "full.sh exited 0: a stale all.jsonl validated a failing run"
else
  report ok "$name" "rejected (exit $rc)"
fi

# A failing case the baseline lists: the runner exits 1, the run is
# complete and its only failure is tolerated, so the lane passes.
name=tolerated-failure
run_full "$name" tolerated-failure
if [ "$rc" -ne 0 ]; then
  report FAIL "$name" "full.sh exited $rc; a complete run with only a listed failure passes"
elif ! grep -q '^== summary ==' "$work/$name.log"; then
  report FAIL "$name" "no summary printed"
else
  report ok "$name" "accepted"
fi

# A complete run passes, writes everything into its own run directory and
# nothing into a shared name; a second run of the same name gets another
# directory.
name=complete
run_full "$name" complete
run_dirs=()
if [ -d "$root/runs" ]; then
  mapfile -t run_dirs < <(find "$root/runs" -mindepth 1 -maxdepth 1 -type d -name "*-$name" | sort)
fi
if [ "$rc" -ne 0 ]; then
  report FAIL "$name" "full.sh exited $rc on a complete run"
elif [ "${#run_dirs[@]}" -ne 1 ]; then
  report FAIL "$name" "expected one run directory under $root/runs, found ${#run_dirs[@]}"
else
  run_dir="${run_dirs[0]}"
  missing=""
  for artifact in sqlite_parity.jsonl sqlite_parity.jsonl.complete.json cases.json validation.txt summary.txt; do
    [ -s "$run_dir/$artifact" ] || missing="$missing $artifact"
  done
  shared="$(find "$root" -maxdepth 1 -name '*.jsonl' | head -n 1)"
  if [ -n "$missing" ]; then
    report FAIL "$name" "run directory lacks:$missing"
  elif [ -n "$shared" ]; then
    report FAIL "$name" "wrote a shared artifact outside its run directory: $shared"
  elif ! grep -q -- '--order alternate' "$work/$name.args"; then
    report FAIL "$name" "the perf lane did not ask the runner for --order alternate: $(cat "$work/$name.args")"
  elif ! grep -q 'eligible cases: *3$' "$run_dir/summary.txt"; then
    report FAIL "$name" "summary does not report 3 eligible cases"
  else
    report ok "$name" "accepted into ${run_dir#"$work"/}"
  fi
  rc=0
  STUB_MODE=complete PERF_ROOT="$root" \
    bash "$repo_root/scripts/perf/full.sh" "$target" "$name" > "$work/$name-again.log" 2>&1 || rc=$?
  count="$(find "$root/runs" -mindepth 1 -maxdepth 1 -type d -name "*-$name" | wc -l | tr -d ' ')"
  if [ "$rc" -ne 0 ] || [ "$count" -ne 2 ]; then
    report FAIL "$name-again" "second run exited $rc with $count run directories; expected 0 and 2"
  else
    report ok "$name-again" "a second run got its own directory"
  fi
fi

# Every run removed its temp root, read-only fixture directories and all,
# whether it passed or not.
leftovers=""
for run in "$work"/root-*/runs/*; do
  [ -d "$run" ] || continue
  for base in /dev/shm "${TMPDIR:-/tmp}"; do
    tmp="$base/redline-testing-perf-full-$(basename "$run")"
    if [ -e "$tmp" ]; then
      leftovers="$leftovers $tmp"
      chmod -R u+rwX "$tmp" 2>/dev/null || true
      rm -rf "$tmp"
    fi
  done
done
if [ -n "$leftovers" ]; then
  report FAIL temp-roots-removed "left behind:$leftovers"
else
  report ok temp-roots-removed "every run removed its temp root"
fi

if [ "$failures" -ne 0 ]; then
  printf '\nfull.sh fail-closed fixtures: %d failed\n' "$failures" >&2
  exit 1
fi
printf '\nfull.sh fail-closed fixtures: all passed\n'
