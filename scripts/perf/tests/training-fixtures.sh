#!/usr/bin/env bash
# Fixtures for the PGO and BOLT training runs (scripts/perf/pgo.sh and
# scripts/perf/bolt.sh) and the durability of perf_run_jsonl.
#
# A training run replays the full sqlite_parity corpus through the runner.
# The committed known-failures baseline lists the cases the current target
# fails, so a training run that does not hand the runner that baseline
# fails on those listed cases. bolt.sh runs under `set -e` and `perf record`
# returns the workload's status, so it then stopped before perf2bolt. The
# target also prints a durability notice on stderr unless
# REDLINEDB_QUIET_DURABILITY is set, and that notice fails every case whose
# stderr the runner compares.
#
# bolt.sh and pgo.sh cd to their Git top level, so each scenario runs copies
# of the perf scripts in a scratch Git repository, with stub llvm-bolt,
# perf2bolt, perf, readelf and uname on PATH and the stub runner
# (stub-redline-testing.sh), which gates on --sqlite-known-failures as the
# real runner does.
#
# Usage: scripts/perf/tests/training-fixtures.sh
# Needs bash, git, jq and sha256sum.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
fixtures_dir="$repo_root/scripts/perf/tests"
work="$(mktemp -d "${TMPDIR:-/tmp}/perf-training-fixtures.XXXXXX")"
real_uname="$(command -v uname)"
# bolt.sh creates its fixed temp root; remove it afterwards only if this
# fixture created it.
bolt_tmp=/dev/shm/redline-testing-bolt
if [ -e "$bolt_tmp" ]; then bolt_tmp=''; fi
cleanup() {
  if [ -n "$bolt_tmp" ]; then rmdir "$bolt_tmp" 2>/dev/null || true; fi
  rm -rf "$work"
}
trap cleanup EXIT

sandbox="$work/repo"
mkdir -p "$sandbox/scripts/perf" "$sandbox/target/release-pgo" "$work/bin" "$work/tools"
git -C "$sandbox" init -q
for script in bolt.sh pgo.sh lib.sh lib-rustflags.sh; do
  cp "$repo_root/scripts/perf/$script" "$sandbox/scripts/perf/$script"
done

cp "$fixtures_dir/stub-redline-testing.sh" "$work/bin/redline-testing"
cat > "$work/bin/sqlite3" <<'STUB'
#!/usr/bin/env bash
echo "3.53.1 2026-01-01 00:00:00 stub (64-bit)"
STUB
cat > "$sandbox/target/release-pgo/redlinedb" <<'STUB'
#!/usr/bin/env bash
echo "redlinedb 0.0.0-training-fixture"
STUB

# Stub tools. perf record runs the workload after `--` and returns its
# status, as perf does; the others write the file they are asked for.
cat > "$work/tools/perf" <<'STUB'
#!/usr/bin/env bash
if [ "${1:-}" = --version ]; then echo "perf version 6.8.stub"; exit 0; fi
while [ $# -gt 0 ] && [ "$1" != -- ]; do shift; done
shift
exec "$@"
STUB
cat > "$work/tools/llvm-bolt" <<'STUB'
#!/usr/bin/env bash
if [ "${1:-}" = --version ]; then echo "  LLVM version 18.1.8"; exit 0; fi
while [ $# -gt 0 ]; do
  if [ "$1" = -o ]; then : > "$2"; shift; fi
  shift
done
STUB
cp "$work/tools/llvm-bolt" "$work/tools/perf2bolt"
cat > "$work/tools/readelf" <<'STUB'
#!/usr/bin/env bash
echo "  [12] .rela.text        RELA"
STUB
cat > "$work/tools/uname" <<STUB
#!/usr/bin/env bash
if [ "\${1:-}" = -m ]; then echo x86_64; exit 0; fi
exec "$real_uname" "\$@"
STUB
chmod +x "$work"/bin/* "$work"/tools/* "$sandbox/target/release-pgo/redlinedb"

# The fixture baseline lists case 10002 failing as a target semantic
# failure, and nothing else.
cat > "$work/known-failures.json" <<'JSON'
{
  "schema_version": "redline-testing-sqlite-known-failures-v1",
  "description": "perf training fixture",
  "failures": [
    {"suite": "sqlite_parity", "case_id": "10002", "name": "CASE_10002",
     "stage": "target_semantic_failure", "reason": "fixture", "owner": "fixture"}
  ]
}
JSON

export REDLINE_TESTING_BIN="$work/bin/redline-testing"
export SQLITE_REF_BIN="$work/bin/sqlite3"
export PERF_KNOWN_FAILURES="$work/known-failures.json"
export STUB_NO_TMP_FIXTURE=1
unset CI_REDLINE_TESTING_BIN PERF_WORKERS || true

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

# bolt.sh over a target whose only failure the baseline lists: the
# training run passes the runner's gate and BOLT writes its binary.
name=bolt-known-failure
rc=0
(cd "$sandbox" && PATH="$work/tools:$PATH" STUB_MODE=tolerated-failure \
  STUB_ARGS_LOG="$work/$name.args" bash scripts/perf/bolt.sh) > "$work/$name.log" 2>&1 || rc=$?
if [ "$rc" -ne 0 ]; then
  report FAIL "$name" "bolt.sh exited $rc on a training run whose only failure is listed"
elif [ ! -f "$sandbox/target/release-pgo/redlinedb.bolt" ]; then
  report FAIL "$name" "bolt.sh wrote no BOLT binary"
elif ! grep -q -- "--sqlite-known-failures $PERF_KNOWN_FAILURES" "$work/$name.args"; then
  report FAIL "$name" "the training run gave the runner no baseline: $(cat "$work/$name.args")"
elif ! grep -q '^durability=normal quiet=1 ' "$work/$name.args"; then
  report FAIL "$name" "the training run is not quiet normal durability: $(cat "$work/$name.args")"
else
  report ok "$name" "trained and rewritten"
fi

# An unlisted failure still stops bolt.sh before perf2bolt.
name=bolt-unlisted-failure
rm -f "$sandbox/target/release-pgo/redlinedb.bolt" "$sandbox/target/redlinedb.bolt.fdata"
rc=0
(cd "$sandbox" && PATH="$work/tools:$PATH" STUB_MODE=unexpected-failure STUB_FAIL_CASE=10000 \
  bash scripts/perf/bolt.sh) > "$work/$name.log" 2>&1 || rc=$?
if [ "$rc" -eq 0 ]; then
  report FAIL "$name" "bolt.sh exited 0 on a training run with an unlisted failure"
elif [ -e "$sandbox/target/redlinedb.bolt.fdata" ]; then
  report FAIL "$name" "bolt.sh reached perf2bolt after a failed training run"
else
  report ok "$name" "stopped (exit $rc)"
fi

# pgo.sh shows the same training command.
name=pgo-dry-run
rc=0
(cd "$sandbox" && bash scripts/perf/pgo.sh --dry-run) > "$work/$name.log" 2>&1 || rc=$?
if [ "$rc" -ne 0 ]; then
  report FAIL "$name" "pgo.sh --dry-run exited $rc"
elif ! grep -q -- "--sqlite-known-failures $PERF_KNOWN_FAILURES" "$work/$name.log"; then
  report FAIL "$name" "the PGO training command gives the runner no baseline"
elif ! grep -q 'REDLINEDB_QUIET_DURABILITY=1' "$work/$name.log"; then
  report FAIL "$name" "the PGO training command leaves the durability notice on"
else
  report ok "$name" "baseline and quiet durability"
fi

# perf_run_jsonl (perf-full) keeps the durability notice off stderr.
name=perf-run-jsonl-quiet
rc=0
(
  cd "$sandbox"
  # shellcheck source=scripts/perf/lib.sh
  . scripts/perf/lib.sh
  PERF_TASKSET_DISABLE=1 STUB_MODE=complete STUB_ARGS_LOG="$work/$name.args" \
    perf_run_jsonl "$sandbox/target/release-pgo/redlinedb" 1 0 "$work/$name.jsonl" "$name"
) > "$work/$name.log" 2>&1 || rc=$?
for base in /dev/shm "${TMPDIR:-/tmp}"; do
  rmdir "$base/redline-testing-perf-$name" 2>/dev/null || true
done
if [ "$rc" -ne 0 ]; then
  report FAIL "$name" "perf_run_jsonl exited $rc"
elif ! grep -q '^durability=normal quiet=1 ' "$work/$name.args"; then
  report FAIL "$name" "the runner saw the durability notice on: $(cat "$work/$name.args")"
else
  report ok "$name" "quiet normal durability"
fi

if [ "$failures" -ne 0 ]; then
  printf '\nperf training fixtures: %d failed\n' "$failures" >&2
  exit 1
fi
printf '\nperf training fixtures: all passed\n'
