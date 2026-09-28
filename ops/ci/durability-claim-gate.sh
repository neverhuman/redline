#!/usr/bin/env bash
# Durability claim gate (workplan R10).
#
# Every `<!-- claim:durability.<mode>.<failure-model> -->` tag in README.md or
# under docs/ is a public durability guarantee. Each one needs a receipt from
# `redlinedb-bench durability-evidence` that covers that mode and failure
# model, passed every scenario, was taken from a clean tree, and was built
# from source whose binary inputs (crates/, Cargo.*, rust-toolchain.toml,
# .cargo/) have not changed between the receipt's commit and the release
# commit. `redlinedb-bench durability-evidence-verify` applies the rules.
# No tags means nothing is claimed, and the gate passes.
#
# The release publish step (ops/ci/publish-github-release.sh) runs this
# before it creates the GitHub release. Pull-request CI does not, because a
# receipt is produced once per release candidate, not per commit.
#
# Usage:
#   bash ops/ci/durability-claim-gate.sh [--root DIR] [--receipts DIR] [--at REV]
#
#   --root      tree whose README.md and docs/ are scanned (default: this repo)
#   --receipts  directory of receipt *.json files
#               (default: <root>/benchmark-results/durability)
#   --at        commit the claims ship in (default: HEAD)
#
# REDLINEDB_BENCH_BIN names a built redlinedb-bench; otherwise it is built.

set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
receipts_dir=""
at="HEAD"
while [ $# -gt 0 ]; do
    case "$1" in
        --root) root=$(cd "$2" && pwd); shift 2 ;;
        --receipts) receipts_dir=$2; shift 2 ;;
        --at) at=$2; shift 2 ;;
        *) printf 'durability-claim-gate: unknown argument %s\n' "$1" >&2; exit 64 ;;
    esac
done
receipts_dir=${receipts_dir:-$root/benchmark-results/durability}

sources=()
[ -f "$root/README.md" ] && sources+=("$root/README.md")
[ -d "$root/docs" ] && sources+=("$root/docs")
if [ ${#sources[@]} -eq 0 ]; then
    echo "durability-claim-gate: no README.md or docs/ under $root; nothing to verify"
    exit 0
fi

tag_pattern='<!-- claim:durability\.[a-z0-9-]+\.[a-z0-9-]+ -->'
# A claim comment written in any other shape would slip past the gate, so it
# fails. Prose that names the tag format outside a comment is not a claim.
malformed=$(grep -rnoE --include='*.md' '<!--[[:space:]]*claim:[^>]*>?' "${sources[@]}" \
    | grep -vE ':<!-- claim:durability\.[a-z0-9-]+\.[a-z0-9-]+ -->$' || true)
if [ -n "$malformed" ]; then
    printf 'durability-claim-gate: malformed claim tags (want %s):\n%s\n' "$tag_pattern" "$malformed" >&2
    exit 1
fi
mapfile -t claims < <(grep -rhoE --include='*.md' "$tag_pattern" "${sources[@]}" \
    | sed -E 's/<!-- claim:(durability\.[a-z0-9-]+\.[a-z0-9-]+) -->/\1/' | sort -u)
if [ ${#claims[@]} -eq 0 ]; then
    echo "durability-claim-gate: no durability claim tags in README.md or docs/; nothing to verify"
    exit 0
fi
printf 'durability-claim-gate: claims: %s\n' "${claims[*]}"

receipts=()
if [ -d "$receipts_dir" ]; then
    mapfile -t receipts < <(find "$receipts_dir" -type f -name '*.json' | sort)
fi
if [ ${#receipts[@]} -eq 0 ]; then
    printf 'durability-claim-gate: README.md/docs claim %s, but %s holds no receipt\n' \
        "${claims[*]}" "$receipts_dir" >&2
    exit 1
fi

bench=${REDLINEDB_BENCH_BIN:-}
if [ -z "$bench" ]; then
    (cd "$root" && cargo build --locked -p redlinedb-bench --bin redlinedb-bench >&2)
    bench="${CARGO_TARGET_DIR:-$root/target}/debug/redlinedb-bench"
fi

args=(durability-evidence-verify --repo "$root" --at "$at")
for claim in "${claims[@]}"; do args+=(--claim "$claim"); done
for receipt in "${receipts[@]}"; do args+=(--receipt "$receipt"); done
"$bench" "${args[@]}"
echo "durability-claim-gate: every durability claim has a passing receipt"
