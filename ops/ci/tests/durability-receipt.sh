#!/usr/bin/env bash
# Offline tests for ops/ci/durability-receipt.sh. redlinedb-bench is a stand-in
# (REDLINEDB_BENCH_BIN) that logs its arguments and exits as told, so the
# script's own contract is what is checked: no receipt fails, a failed
# verification fails and leaves no evidence directory, and a passing one
# verifies every committed receipt against the claim at HEAD and hands them on.
#
# Usage: bash ops/ci/tests/durability-receipt.sh
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
script=$root/ops/ci/durability-receipt.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }

cat > "$work/bench" <<BIN
#!/bin/sh
printf '%s\n' "\$@" > "$work/bench.args"
echo "stand-in verifier: exit \${BENCH_EXIT:-0}"
exit "\${BENCH_EXIT:-0}"
BIN
chmod +x "$work/bench"
# run <label> <receipts dir> [VAR=value...]; sets $status.
run() {
  local label=$1 receipts=$2
  shift 2
  rm -f "$work/bench.args"
  status=0
  env REDLINEDB_BENCH_BIN="$work/bench" "$@" bash "$script" "$work/out-$label" --receipts "$receipts" \
    > "$work/$label.log" 2>&1 || status=$?
}

# No receipt: refused before the verifier runs.
mkdir -p "$work/empty"
printf 'not a receipt\n' > "$work/empty/README.md"
run empty "$work/empty"
[[ $status != 0 ]] || fail "empty: accepted a directory without receipts"
grep -qF 'no durability receipt' "$work/empty.log" || fail "empty: $(tail -n 1 "$work/empty.log")"
[[ ! -e $work/bench.args ]] || fail "empty: ran the verifier without a receipt"
run missing "$work/does-not-exist"
[[ $status != 0 ]] || fail "missing: accepted a missing receipts directory"

mkdir -p "$work/receipts"
printf '{"receipt":"b"}\n' > "$work/receipts/b.json"
printf '{"receipt":"a"}\n' > "$work/receipts/a.json"
printf 'notes\n' > "$work/receipts/notes.txt"

# A failed verification fails the job and leaves nothing to upload.
run refused "$work/receipts" BENCH_EXIT=1
[[ $status != 0 ]] || fail "refused: passed although the verifier failed"
[[ ! -e $work/out-refused ]] || fail "refused: left an evidence directory to upload"
grep -qF 'durability-evidence-verify exited 1' "$work/refused.log" || fail "refused: $(tail -n 1 "$work/refused.log")"

# A passing verification: every receipt, the default claim, at HEAD.
run passed "$work/receipts"
[[ $status == 0 ]] || fail "passed: $(tail -n 2 "$work/passed.log")"
expected=$(printf '%s\n' durability-evidence-verify --repo "$root" --at HEAD \
  --claim durability.strict.process-kill --receipt "$work/receipts/a.json" --receipt "$work/receipts/b.json")
[[ $(cat "$work/bench.args" 2>/dev/null) == "$expected" ]] ||
  fail "passed: verifier arguments were: $(tr '\n' ' ' < "$work/bench.args" 2>/dev/null)"
[[ -f $work/out-passed/receipts/a.json && -f $work/out-passed/receipts/b.json ]] || fail "passed: receipts not handed on"
[[ ! -e $work/out-passed/receipts/notes.txt ]] || fail "passed: a non-receipt file was handed on"
grep -qF 'stand-in verifier' "$work/out-passed/verify.log" || fail "passed: the verifier's output was not kept"
[[ $(cat "$work/out-passed/claims.txt") == durability.strict.process-kill ]] || fail "passed: claims.txt"
[[ $(cat "$work/out-passed/commit.txt") == "$(git -C "$root" rev-parse HEAD)" ]] || fail "passed: commit.txt"

# Explicit claims replace the default.
status=0
env REDLINEDB_BENCH_BIN="$work/bench" bash "$script" "$work/out-claims" --receipts "$work/receipts" \
  --claim durability.normal.process-kill --claim durability.strict.process-kill > "$work/claims.log" 2>&1 || status=$?
[[ $status == 0 ]] || fail "claims: $(tail -n 1 "$work/claims.log")"
claims=$(awk 'previous == "--claim" { printf "%s ", $0 } { previous = $0 }' "$work/bench.args")
[[ $claims == "durability.normal.process-kill durability.strict.process-kill " ]] ||
  fail "claims: verifier got claims '$claims'"

((failures == 0)) || { printf '%d durability receipt check(s) failed\n' "$failures" >&2; exit 1; }
printf 'Durability receipt tests passed.\n'
