#!/usr/bin/env bash
# Launch claim lint (WP-0.1). Every tracked line of README.md and docs/ (not
# docs/archive/ or docs/migration/; docs/releases/ is included) that says
# "drop-in", "100% SQLite", "full SQLite compat", "100% safe-Rust" or
# "faster than SQLite" must be a reviewed line: its fingerprint is in
# scripts/launch-claims-allowlist.tsv with the reason it may stand, either
# `qualified` (the line itself negates or scopes the claim) or `historical`
# (the line reports a dated measurement or plan and is marked as such).
#
#   bash scripts/check-launch-claims.sh               # check; exit 1 on any finding
#   bash scripts/check-launch-claims.sh --print       # every hit as an allowlist row
#   bash scripts/check-launch-claims.sh --root <dir>  # check another checkout
#
# A fingerprint is the first 16 hex digits of sha256("<path>\t<text>"), where
# <text> is the line with surrounding blanks trimmed, inner runs of blanks
# folded to one space and every run of digits written as 0. Regenerated counts
# keep a reviewed line reviewed; any change to its words, or a move to another
# file, needs a new review. An allowlist row whose line no longer exists fails
# too, so the list only holds lines that are still there.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
mode=check
while [[ $# -gt 0 ]]; do
  case $1 in
    --print) mode=print; shift ;;
    --root) root=$(cd "$2" && pwd); shift 2 ;;
    *) printf 'usage: check-launch-claims.sh [--print] [--root <dir>]\n' >&2; exit 2 ;;
  esac
done
cd "$root"
allowlist=scripts/launch-claims-allowlist.tsv
# drop-in must end the word: `create-drop-index` is not a claim.
pattern='drop-in([^[:alnum:]_]|$)|100% SQLite|full SQLite compat|100% safe-Rust|faster than SQLite'

fingerprint() {
  local text
  text=$(sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//; s/[[:space:]]+/ /g; s/[0-9]+/0/g' <<<"$2")
  printf '%s\t%s' "$1" "$text" | sha256sum | cut -c1-16
}

declare -A reviewed=() seen=()
if [[ -f $allowlist ]]; then
  while IFS=$'\t' read -r print path kind reason; do
    [[ -z $print || $print == \#* ]] && continue
    case $kind in
      qualified | historical) ;;
      *) printf 'launch-claims: %s: row %s has kind "%s"; use qualified or historical\n' \
        "$allowlist" "$print" "$kind" >&2; exit 1 ;;
    esac
    [[ -n $reason ]] || { printf 'launch-claims: %s: row %s has no reason\n' "$allowlist" "$print" >&2; exit 1; }
    reviewed[$print]=$path
  done <"$allowlist"
fi

findings=0
# git grep exits 1 when nothing matches.
hits=$(git grep -n -I -i -E "$pattern" -- README.md docs \
  ':(exclude)docs/archive' ':(exclude)docs/migration') || true
while IFS= read -r hit; do
  [[ -n $hit ]] || continue
  path=${hit%%:*}
  rest=${hit#*:}
  number=${rest%%:*}
  text=${rest#*:}
  print=$(fingerprint "$path" "$text")
  seen[$print]=1
  if [[ $mode == print ]]; then
    printf '%s\t%s\t<qualified|historical>\t<reason> (line %s: %.80s)\n' "$print" "$path" "$number" "$text"
  elif [[ -z ${reviewed[$print]:-} ]]; then
    printf 'launch-claims: %s:%s: unreviewed claim [%s]: %.160s\n' "$path" "$number" "$print" "$text" >&2
    findings=$((findings + 1))
  fi
done <<<"$hits"
[[ $mode == print ]] && exit 0

for print in "${!reviewed[@]}"; do
  if [[ -z ${seen[$print]:-} ]]; then
    printf 'launch-claims: %s: row %s (%s) matches no line; remove it\n' \
      "$allowlist" "$print" "${reviewed[$print]}" >&2
    findings=$((findings + 1))
  fi
done

if ((findings)); then
  printf 'launch-claims: %d finding(s). Qualify or mark the line, then add a reviewed row with --print.\n' \
    "$findings" >&2
  exit 1
fi
printf 'launch-claims: ok (%d reviewed line(s))\n' "${#seen[@]}"
