#!/usr/bin/env bash
# Fixtures for scripts/perf/release-bench.sh and the argument checks of
# scripts/perf/build-version.sh (L-05).
#
# Each scenario runs the real release-bench.sh against the stub runner
# (stub-redline-testing.sh), a stub sqlite3 and stub targets, with a fake
# load average, and checks its exit status and the bundle it leaves. A
# bundle is written only when every run is the complete requested
# experiment, accepted under the load threshold, with any non-zero runner
# exit explained by a failed case; release binaries only.
#
# Usage: scripts/perf/tests/release-bench-fixtures.sh
# Needs bash, jq, sha256sum and taskset. PERF_EVIDENCE_BIN may name a built
# perf_evidence binary; otherwise the scripts run it through cargo.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
fixtures_dir="$repo_root/scripts/perf/tests"
bench="$repo_root/scripts/perf/release-bench.sh"
work="$(mktemp -d "${TMPDIR:-/tmp}/release-bench-fixtures.XXXXXX")"
trap 'chmod -R u+rwX "$work" 2>/dev/null; rm -rf "$work"' EXIT

mkdir -p "$work/bin" "$work/shm" "$work/a" "$work/b" "$work/fp" "$work/dbg" "$work/wrong"
cp "$fixtures_dir/stub-redline-testing.sh" "$work/bin/redline-testing"
cat > "$work/bin/sqlite3" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
  --version) echo "3.53.1 2026-01-01 00:00:00 stub (64-bit)" ;;
  *) printf 'COMPILER=stub\nTHREADSAFE=1\n' ;;
esac
EOF
# Target a has the durability knob; target b does not.
target() {
  local path="$1" extra="$2"
  printf '#!/usr/bin/env bash\n# %s\necho "redlinedb %s"\n' "$extra" "$(basename "$(dirname "$path")")" > "$path"
  chmod +x "$path"
}
target "$work/a/redlinedb" "reads REDLINEDB_DEFAULT_DURABILITY"
target "$work/b/redlinedb" "an old version"
target "$work/fp/redlinedb" "fail point wal::write_encoded"
target "$work/dbg/redlinedb" "unsafe precondition(s) violated: slice"
target "$work/wrong/redlinedb" "reads REDLINEDB_DEFAULT_DURABILITY; mislabelled"
chmod +x "$work/bin/redline-testing" "$work/bin/sqlite3"
# b was built by build-version.sh: its build.json names it.
jq -n --arg sha "$(sha256sum "$work/b/redlinedb" | awk '{print $1}')" \
  '{schema_version: "redline-version-build-v1", label: "b", source_ref: "v0.0.1",
    source_commit: "0123456789abcdef0123456789abcdef01234567",
    rustc_verbose_version: ["rustc 1.95.0 (fixture)"], profile: "release",
    features: "default", rustflags: "", pgo: false, binary_sha256: $sha}' \
  > "$work/b/build.json"
jq -n '{schema_version: "redline-version-build-v1", source_commit: "00", binary_sha256: "00",
  rustc_verbose_version: [], profile: "release", features: "default", rustflags: "", pgo: false}' \
  > "$work/wrong/build.json"
printf '# fixture cohort\n10000\n10001  # note\n' > "$work/cohort.txt"
printf '1.00 1.00 1.00 1/100 1\n' > "$work/loadavg"

export REDLINE_TESTING_BIN="$work/bin/redline-testing"
export SQLITE_REF_BIN="$work/bin/sqlite3"
export RELEASE_BENCH_TMPFS="$work/shm"
export RELEASE_BENCH_LOADAVG="$work/loadavg"
export RELEASE_BENCH_JOB_PATTERN="no-such-runner-job-for-release-bench-fixtures"
export RELEASE_BENCH_RUNNER_UNITS="no-such-unit-for-release-bench-fixtures"
unset CI_REDLINE_TESTING_BIN STUB_MODE STUB_CASES STUB_FAIL_CASE || true
out="$work/out"

failures=0
report() {
  local verdict="$1" name="$2" detail="$3"
  printf '%-4s %-36s %s\n' "$verdict" "$name" "$detail"
  if [ "$verdict" != ok ]; then
    failures=$((failures + 1))
    if [ -f "$work/$name.log" ]; then
      sed 's/^/       | /' "$work/$name.log"
    fi
  fi
}

# bench <name> [args...]: runs release-bench.sh; sets rc and dir.
bench() {
  local name="$1"
  shift
  dir="$out/$name"
  rc=0
  STUB_ARGS_LOG="$work/$name.args" bash "$bench" --bundle "$name" --out-root "$out" \
    --cpus 0 --medium-cohort "$work/cohort.txt" "$@" > "$work/$name.log" 2>&1 || rc=$?
}

# expect <name> <exit> <condition> <detail>
expect() {
  local name="$1" code="$2" condition="$3" detail="$4"
  if [ "$rc" -eq "$code" ] && eval "$condition"; then
    report ok "$name" "$detail"
  else
    report FAIL "$name" "exit $rc (want $code); $detail"
  fi
}

summary() { jq -r "$1" "$dir/summary.json"; }

# 1. A complete two-label bundle, two runs each, interleaved and rotated.
bench complete --runs 2 a="$work/a/redlinedb" b="$work/b/redlinedb"
expect complete 0 '
  [ -f "$dir/bundle.json" ] && [ -f "$dir/host.json" ] && [ -f "$dir/cases.json" ] &&
  [ -f "$dir/README.md" ] && [ -f "$dir/a/build-contract.json" ] && [ -f "$dir/b/build.json" ] &&
  [ -f "$dir/a/run-2/raw.jsonl" ] && [ -f "$dir/b/run-1/raw.jsonl.complete.json" ] &&
  [ "$(summary .common_pass_set.cases)" = 3 ] && [ "$(summary .runs_per_label)" = 2 ] &&
  [ "$(summary "[.labels[].label] | join(\",\")")" = a,b ] &&
  [ "$(jq -r "[.runs | sort_by(.sequence)[] | \"\(.label)\(.run)\"] | join(\",\")" "$dir/bundle.json")" = a1,b1,b2,a2 ] &&
  [ "$(jq "[.runs[] | select(.accepted)] | length" "$dir/host.json")" = 4 ] &&
  [ "$(summary .publishable)" = false ] &&
  [ "$(summary ".labels[1].build.rustflags")" = "" ] &&
  [ "$(summary ".labels[1].source_commit")" = 0123456789abcdef0123456789abcdef01234567 ] &&
  [ "$(summary ".labels[0].durability")" = normal ] &&
  [ "$(summary ".labels[1].durability")" = "built-in default" ] &&
  summary ".publication_blockers[]" | grep -q "b: the binary has no REDLINEDB_DEFAULT_DURABILITY knob" &&
  summary ".publication_blockers[]" | grep -q "a: the build is undeclared" &&
  [ "$(grep -c -- "--workers 1 --repetitions 3 --warmup 1 --order alternate --tmp-root $work/shm/rl-bench-complete-" "$work/complete.args")" = 4 ] &&
  [ "$(grep -c "^durability=normal " "$work/complete.args")" = 4 ] &&
  [ -z "$(ls -A "$work/shm")" ] &&
  [ "$(jq -r .tmp_filesystem "$dir/host.json")" = "$(stat -f -c %T "$work/shm")" ] &&
  grep -q cli_case_wall_time "$dir/README.md" &&
  "$PERF_EVIDENCE_BIN" summarize-bundle --bundle "$dir" --check > /dev/null
' "bundle, contracts, rotation a1,b1,b2,a2, summary, blockers, temp roots removed"

# 2. The same bundle name again is refused before anything runs.
rm -f "$work/again.args"
rc=0
STUB_ARGS_LOG="$work/again.args" bash "$bench" --bundle complete --out-root "$out" --cpus 0 \
  a="$work/a/redlinedb" > "$work/again.log" 2>&1 || rc=$?
expect again 2 '[ ! -f "$work/again.args" ] && grep -q "already exists" "$work/again.log"' \
  "an existing bundle directory is never reused"

# 3-4. A failpoints build or a debug build is not a release measurement.
bench failpoints a="$work/fp/redlinedb"
expect failpoints 2 '[ ! -e "$dir" ] && grep -q "wal::write_encoded" "$work/failpoints.log"' \
  "failpoint names are refused"
bench debug a="$work/dbg/redlinedb"
expect debug 2 '[ ! -e "$dir" ] && grep -q "debug assertions" "$work/debug.log"' \
  "debug-assertion markers are refused"

# 5. A build.json beside a binary must describe that binary.
bench mislabelled a="$work/wrong/redlinedb"
expect mislabelled 2 '[ ! -e "$dir" ] && grep -q "does not describe" "$work/mislabelled.log"' \
  "a build record for another binary is refused"

# 6. A busy host rejects the run and leaves no bundle.json.
printf '40.00 1.00 1.00 1/100 1\n' > "$work/loadavg"
bench overloaded --max-loadavg 32 a="$work/a/redlinedb"
printf '1.00 1.00 1.00 1/100 1\n' > "$work/loadavg"
expect overloaded 3 '
  [ ! -f "$dir/bundle.json" ] && [ "$(jq ".runs[0].accepted" "$dir/host.json")" = false ] &&
  jq -r ".runs[0].reason" "$dir/host.json" | grep -q "exceeds 32" && [ ! -f "$work/overloaded.args" ]
' "load 40 > 32: rejected before the run, no bundle.json"

# 7. An incomplete run is rejected.
STUB_MODE=missing-repetition bench incomplete a="$work/a/redlinedb"
expect incomplete 3 '[ ! -f "$dir/bundle.json" ] && grep -q "not the complete requested run" "$work/incomplete.log"' \
  "a missing repetition rejects the run"

# 8. A non-zero runner exit that no failed case explains is rejected.
STUB_MODE=nonzero-without-failure bench unexplained a="$work/a/redlinedb"
expect unexplained 3 '[ ! -f "$dir/bundle.json" ] && grep -q "no case failed" "$work/unexplained.log"' \
  "runner exit 1 with every case passed"

# 9. A failed case is data: the run is accepted and the case leaves the common set.
STUB_MODE=tolerated-failure bench failing a="$work/a/redlinedb"
expect failing 0 '
  [ "$(summary ".labels[0].passed_cases")" = 2 ] && [ "$(summary .common_pass_set.cases)" = 2 ] &&
  [ "$(summary ".labels[0].runs[0].runner_exit")" = 1 ]
' "case 10002 fails every run: 2 passed, common pass set 2"

# 10. A case list narrows the runs and the summary says so.
printf '10000\n10002\n' > "$work/list.txt"
bench narrowed --runs 1 --case-list "$work/list.txt" a="$work/a/redlinedb"
expect narrowed 0 '
  grep -q -- "--case-id 10000 --case-id 10002" "$work/narrowed.args" &&
  [ "$(jq length "$dir/cases.json")" = 2 ] && [ -f "$dir/case-list.txt" ] &&
  summary ".publication_blockers[0]" | grep -q "narrowed to 2 cases"
' "--case-id per listed case; narrowed summary is not publishable"

# 11. A case list naming a case the corpus lacks is refused before anything runs.
printf '10009\n' > "$work/unknown.txt"
bench unknown-case --case-list "$work/unknown.txt" a="$work/a/redlinedb"
expect unknown-case 2 '[ ! -e "$dir" ] && grep -q "does not have: 10009" "$work/unknown-case.log"' \
  "unknown case id"

# 12. --durability default exports no override.
bench default-durability --runs 1 --durability default a="$work/a/redlinedb" b="$work/b/redlinedb"
expect default-durability 0 '
  [ "$(grep -c "^durability= " "$work/default-durability.args")" = 2 ] &&
  [ "$(summary "[.labels[].durability] | unique | join(\",\")")" = "built-in default" ] &&
  ! summary ".publication_blockers[]" | grep -q "REDLINEDB_DEFAULT_DURABILITY"
' "no REDLINEDB_DEFAULT_DURABILITY; every label at its built-in default"

# 13-15. build-version.sh refuses bad input before it clones anything.
rc=0
bash "$repo_root/scripts/perf/build-version.sh" no-such-ref-for-fixtures fixture > "$work/bv-ref.log" 2>&1 || rc=$?
expect build-version-ref 2 'grep -q "names no commit" "$work/bv-ref.log" && [ ! -e "$repo_root/.agent/sandbox/fixture" ]' \
  "an unknown ref"
rc=0
bash "$repo_root/scripts/perf/build-version.sh" HEAD 'bad/label' > "$work/bv-label.log" 2>&1 || rc=$?
expect build-version-label 2 'grep -q "label must match" "$work/bv-label.log"' "a label with a slash"
rc=0
VERSION_BUILD_RUSTFLAGS='-Cprofile-use=/x.profdata' \
  bash "$repo_root/scripts/perf/build-version.sh" HEAD fixture > "$work/bv-pgo.log" 2>&1 || rc=$?
expect build-version-pgo 2 'grep -q "must not use a PGO profile" "$work/bv-pgo.log"' "PGO flags"

if [ "$failures" -ne 0 ]; then
  printf '\nrelease-bench fixtures: %d failed\n' "$failures"
  exit 1
fi
printf '\nrelease-bench fixtures: all passed\n'
