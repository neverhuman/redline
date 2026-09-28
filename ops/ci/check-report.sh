#!/usr/bin/env bash
# Prove the included reporter consumes the complete, verified parity evidence.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp README.md "$work/README.md"
# Official mode: the run's own provenance, which must record a clean source
# tree (the measured binaries are then the checked-out commit). Work in
# progress under the source inputs fails this check; see docs/testing.md.
target/release/redline-testing report \
  --suite sqlite_parity \
  --input target/redline-testing/sqlite_parity.raw.jsonl \
  --official-evidence target/redline-testing/official-evidence.processed.json \
  --run-provenance target/redline-testing/provenance.json \
  --out-dir "$work/report" --readme "$work/README.md" \
  --updated-date "$(date -u +%F)" \
  --expected-repetitions "${REDLINEDB_SQLITE_PARITY_REPETITIONS:-3}" \
  --expected-warmup "${REDLINEDB_SQLITE_PARITY_WARMUP:-1}"
# Writing even a scratch README needs release evidence: the checked-out
# commit, a clean source tree when the run started, and a measured reference.
target/release/redline-testing check-postgres \
  --input target/redline-testing/beyond_sqlite.raw.jsonl \
  --baseline metadata/beyond_sqlite/postgres-regression.json \
  --readme "$work/README.md" \
  --expected-source-commit "$(git rev-parse HEAD)"
printf 'Verified SQLite and PostgreSQL corpus report generation passed.\n'
