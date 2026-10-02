#!/usr/bin/env bash
# Offline tests for ops/ci/durability-claim-gate.sh. redlinedb-bench is a
# stand-in (REDLINEDB_BENCH_BIN) that logs its arguments and exits as told.
# A custody inventory is not a receipt and must not reach the verifier.
#
# Usage: bash ops/ci/tests/durability-claim-gate.sh
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
script=$root/ops/ci/durability-claim-gate.sh
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
mkdir -p "$work/tree" "$work/receipts"
printf '<!-- claim:durability.strict.process-kill -->\n' > "$work/tree/README.md"
printf '{"schema_version":1,"receipt":"a"}\n' > "$work/receipts/a.json"
printf '{"schema":"redline-durability-raw-custody-v1"}\n' > "$work/receipts/custody.json"

status=0
env REDLINEDB_BENCH_BIN="$work/bench" bash "$script" \
  --root "$work/tree" --receipts "$work/receipts" --at HEAD \
  > "$work/passed.log" 2>&1 || status=$?
[[ $status == 0 ]] || fail "passed: $(tail -n 1 "$work/passed.log")"
grep -q 'receipts/a.json' "$work/bench.args" || fail "passed: the receipt was not verified"
grep -q custody.json "$work/bench.args" && fail "passed: the verifier was given the custody inventory"

rm -f "$work/receipts/a.json"
status=0
env REDLINEDB_BENCH_BIN="$work/bench" bash "$script" \
  --root "$work/tree" --receipts "$work/receipts" --at HEAD \
  > "$work/only-custody.log" 2>&1 || status=$?
[[ $status != 0 ]] || fail "only-custody: a custody inventory counted as a receipt"
grep -qF 'holds no receipt' "$work/only-custody.log" || fail "only-custody: $(tail -n 1 "$work/only-custody.log")"

((failures == 0)) || { printf '%d durability claim gate check(s) failed\n' "$failures" >&2; exit 1; }
printf 'Durability claim gate tests passed.\n'
