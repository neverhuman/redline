#!/usr/bin/env bash
# Every package in the root cargo workspace must be private: `publish = false`
# in [workspace.package] and `publish.workspace = true` in each member. Cargo
# then refuses `cargo publish`, and cargo-deny's `allow-wildcard-paths` accepts
# the versionless path dependencies between members. RedlineDB is not on
# crates.io; docs/api-stability.md says how to depend on it.
#
#   scripts/check-publish-policy.sh [repository-root]
set -euo pipefail
root=${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}
command -v jq >/dev/null || { printf 'FAIL: jq is required\n' >&2; exit 1; }
metadata=$(cargo metadata --no-deps --format-version 1 --locked --manifest-path "$root/Cargo.toml")
# `publish = false` reports as []; an absent key reports as null, and a
# registry list means the package may be published there.
publishable=$(jq -r '.packages[] | select(.publish != []) | "\(.name) (publish = \(.publish | tojson))"' <<<"$metadata")
if [[ -n $publishable ]]; then
  printf 'FAIL: workspace packages that cargo would publish:\n%s\n' "$publishable" >&2
  printf 'Set publish.workspace = true in each member; [workspace.package] has publish = false.\n' >&2
  exit 1
fi
count=$(jq '.packages | length' <<<"$metadata")
printf 'Publish policy checks passed: %s workspace packages are publish = false.\n' "$count"
