#!/usr/bin/env bash
# The immutable published CLI is the runtime documentation oracle.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
target_directory=$(node -e 'console.log(require("node:path").resolve(process.argv[1]))' "${CARGO_TARGET_DIR:-$root/target}")
cache="$target_directory/docs-reference"
evidence="$target_directory/docs-check"
docs_scratch=$(node -e 'console.log(require("node:path").resolve(process.argv[1]))' "${TMPDIR:-$evidence/tmp}")
case "$docs_scratch" in /tmp|/tmp/*) docs_scratch="$evidence/tmp" ;; esac
mkdir -p "$evidence" "$docs_scratch"
export TMPDIR="$docs_scratch"
node scripts/docs/fetch-release.mjs --cache "$cache" > "$evidence/oracles.json"
docs_binary=$(node -e 'const r=require(process.argv[1]); console.log(r.receipts.find(x=>x.tag==="v5.1.1").binary)' "$evidence/oracles.json")
docs_old_binary=$(node -e 'const r=require(process.argv[1]); console.log(r.receipts.find(x=>x.tag==="v5.1.0").binary)' "$evidence/oracles.json")
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-line-tables-only}"
cargo build --locked -p redlinedb --lib
node --test scripts/docs/check-hardening.test.mjs
REDLINE_DOCS_TEST_BINARY="$docs_binary" REDLINE_DOCS_TEST_OLD_BINARY="$docs_old_binary" node --test scripts/docs/check.test.mjs
node scripts/docs/check.mjs --oracles "$evidence/oracles.json" \
  --rlib "$target_directory/debug/libredlinedb.rlib" --output "$evidence/receipt.json"
docs_packages=$(node -e 'const p=require("node:path");const r=require(process.argv[1]); console.log(p.dirname(r.receipts.find(x=>x.tag==="v5.1.1").archive))' "$evidence/oracles.json")
bash scripts/test-docs-quickstart.sh "$docs_packages"
ABI_PROBE_OUT="$evidence/abi" bash scripts/test-package-ffi.sh "$docs_packages"
