#!/usr/bin/env bash
# Prove the included reporter consumes the complete, verified parity evidence.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp README.md "$work/README.md"
target/release/redline-testing report \
  --suite sqlite_parity \
  --input target/redline-testing/sqlite_parity.raw.jsonl \
  --official-evidence target/redline-testing/official-evidence.processed.json \
  --out-dir "$work/report" --readme "$work/README.md" \
  --updated-date "$(date -u +%F)" \
  --expected-repetitions "${REDLINEDB_SQLITE_PARITY_REPETITIONS:-3}" \
  --expected-warmup "${REDLINEDB_SQLITE_PARITY_WARMUP:-1}"
target/release/redline-testing check-postgres \
  --input target/redline-testing/beyond_sqlite.raw.jsonl \
  --baseline metadata/beyond_sqlite/postgres-regression.json \
  --readme "$work/README.md"
printf 'Verified SQLite and PostgreSQL corpus report generation passed.\n'
