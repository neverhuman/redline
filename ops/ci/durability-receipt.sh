#!/usr/bin/env bash
# The durability-evidence release receipt (ops/release/acceptance-receipts).
#
# Durability receipts are taken once per release candidate, on a real disk,
# with `redlinedb-bench durability-evidence` against the shipped shell, and
# committed under benchmark-results/durability/ (docs/manual/durability.md,
# "Receipts"). This script checks the committed receipts against the claims
# with `redlinedb-bench durability-evidence-verify --repo . --at HEAD`: a
# receipt backs a claim only when it passed every scenario and was taken from
# a clean tree whose commit is an ancestor of HEAD with no change to the
# binary's inputs in between. It fails when there is no receipt or the
# verification fails; otherwise it copies the receipts and the verifier's
# output into <out dir>, which ci.yml uploads as the durability-evidence
# artifact of a tag run.
#
#   ops/ci/durability-receipt.sh <out dir> [--receipts DIR] [--claim CLAIM]...
#
#   --receipts  directory of receipt *.json files
#               (default: benchmark-results/durability)
#   --claim     durability.<mode>.<failure-model> to verify; repeat for
#               several (default: durability.strict.process-kill)
#
# REDLINEDB_BENCH_BIN names a built redlinedb-bench; otherwise it is built
# with `cargo build --locked -p redlinedb-bench`.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
die() { printf 'durability-receipt: %s\n' "$*" >&2; exit 1; }

(($#)) || die "usage: durability-receipt.sh <out dir> [--receipts DIR] [--claim CLAIM]..."
out=$1
shift
receipts_dir=$root/benchmark-results/durability
claims=()
while (($#)); do
  case $1 in
    --receipts) receipts_dir=$2; shift 2 ;;
    --claim) claims+=("$2"); shift 2 ;;
    *) die "unknown argument $1" ;;
  esac
done
((${#claims[@]})) || claims=(durability.strict.process-kill)

receipts=()
if [[ -d $receipts_dir ]]; then
  while IFS= read -r receipt; do
    receipts+=("$receipt")
  done < <(find "$receipts_dir" -maxdepth 1 -type f -name '*.json' | LC_ALL=C sort)
fi
((${#receipts[@]})) ||
  die "no durability receipt in $receipts_dir: take one with redlinedb-bench durability-evidence on a real disk (docs/manual/durability.md, \"Receipts\") and commit it"

bench=${REDLINEDB_BENCH_BIN:-}
if [[ -z $bench ]]; then
  (cd "$root" && cargo build --locked -p redlinedb-bench --bin redlinedb-bench >&2)
  bench=${CARGO_TARGET_DIR:-$root/target}/debug/redlinedb-bench
fi

args=(durability-evidence-verify --repo "$root" --at HEAD)
for claim in "${claims[@]}"; do args+=(--claim "$claim"); done
for receipt in "${receipts[@]}"; do args+=(--receipt "$receipt"); done

rm -rf "$out"
mkdir -p "$out/receipts"
status=0
"$bench" "${args[@]}" > "$out/verify.log" 2>&1 || status=$?
cat "$out/verify.log"
if ((status != 0)); then
  rm -rf "$out"
  die "durability-evidence-verify exited $status; no receipt backs ${claims[*]} at $(git -C "$root" rev-parse HEAD)"
fi
cp "${receipts[@]}" "$out/receipts/"
printf '%s\n' "${claims[@]}" > "$out/claims.txt"
git -C "$root" rev-parse HEAD > "$out/commit.txt"
printf 'durability-receipt: %d receipt(s) back %s\n' "${#receipts[@]}" "${claims[*]}"
