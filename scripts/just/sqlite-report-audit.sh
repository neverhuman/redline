#!/usr/bin/env bash
# Score this checkout for a parity report without changing tracked audit data.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
cd "$repo_root"

output_dir="${1:-target/sqlite-parity-report-audit}"
case "$output_dir" in
  target/*) ;;
  *) printf 'report audit output must be under target/: %s\n' "$output_dir" >&2; exit 2 ;;
esac
case "/$output_dir/" in
  */../*|*/./*) printf 'report audit output contains a traversal: %s\n' "$output_dir" >&2; exit 2 ;;
esac
if [[ -L target ]]; then
  printf 'report audit requires a real target/ directory in the checkout\n' >&2
  exit 2
fi
mkdir -p target
target_root="$(realpath target)"
if [[ "$target_root" != "$repo_root/target" ]]; then
  printf 'report audit requires a real target/ directory in the checkout\n' >&2
  exit 2
fi
output_root="$(realpath -m "$output_dir")"
case "$output_root/" in
  "$target_root/"*) ;;
  *) printf 'report audit output resolves outside target/: %s\n' "$output_dir" >&2; exit 2 ;;
esac
mkdir -p "$output_dir"

bash scripts/check_audit_policy_mirror.sh
jankurai audit . --mode advisory \
  --json "$output_dir/repo-score.json" \
  --md "$output_dir/repo-score.md" \
  --no-score-history --policy agent/audit-policy.toml >&2
printf '%s\n' "$output_dir/repo-score.json"
