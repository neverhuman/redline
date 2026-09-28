#!/usr/bin/env bash
# Render the feature tables of docs/sqlite-parity.md from
# docs/sqlite-feature-matrix.json. The tables sit between
#   <!-- sqlite-feature-matrix:begin -->  and  <!-- sqlite-feature-matrix:end -->
# and are never edited by hand: edit the JSON and run this script.
#
#   bash scripts/parity/render-sqlite-feature-matrix.sh          # rewrite the block
#   bash scripts/parity/render-sqlite-feature-matrix.sh --check  # exit 1 if it differs
#   ... [--matrix <json>] [--ledger <md>]                        # other files (tests)
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
matrix=$repo_root/docs/sqlite-feature-matrix.json
ledger=$repo_root/docs/sqlite-parity.md
check=0
while [[ $# -gt 0 ]]; do
  case $1 in
    --check) check=1; shift ;;
    --matrix) matrix=$2; shift 2 ;;
    --ledger) ledger=$2; shift 2 ;;
    *) printf 'usage: render-sqlite-feature-matrix.sh [--check] [--matrix F] [--ledger F]\n' >&2; exit 2 ;;
  esac
done
begin='<!-- sqlite-feature-matrix:begin -->'
end='<!-- sqlite-feature-matrix:end -->'

block=$(jq -r '
  def cell: tostring | gsub("\\|"; "\\|") | gsub("\n"; " ");
  def code: "`" + . + "`";
  # Consecutive case ids collapse to first–last.
  def ranges:
    reduce .[] as $id ([];
      if length > 0 and ((.[-1].last | tonumber) + 1) == ($id | tonumber)
      then .[-1].last = $id else . + [{first: $id, last: $id}] end)
    | map(if .first == .last then .first else "\(.first)–\(.last)" end) | join(", ");
  def tests_cell:
    ([.tests[] | capture("^(?<path>[^:]+)::(?<fn>.+)$")]
      | group_by(.path) | sort_by(.[0].path)
      | map((.[0].path | code) + ": " + (map(.fn | code) | join(", "))))
    + (.artifacts | map(code))
    | if length == 0 then "none" else join("; ") end;
  def notes_cell:
    .notes
    + (if (.corpus_case_ids | length) > 0 then " Corpus cases: " + (.corpus_case_ids | ranges) + "." else "" end)
    + (if (.open_subfeatures | length) > 0 then " Open: " + (.open_subfeatures | join("; ")) + "." else "" end)
    | ltrimstr(" ");
  .features as $rows
  | ($rows | map(.section) | reduce .[] as $s ([]; if index([$s]) then . else . + [$s] end)) as $sections
  | [$sections[] as $section
      | "## \($section)", "",
        "| Feature row | Status | Proof | Tests | Owner | Notes |",
        "|---|---|---|---|---|---|",
        ($rows[] | select(.section == $section)
          | "| \(.feature | cell) | \(.status) | \(.proof_kind) | \(tests_cell | cell) | \(.owner | cell) | \(notes_cell | cell) |"),
        ""]
  | .[:-1] | join("\n")
' "$matrix")

# ENVIRON, not -v: awk would read the backslashes in `\|` as escapes.
rendered=$(BLOCK=$block awk -v begin="$begin" -v end="$end" '
  $0 == begin { print; print ""; print ENVIRON["BLOCK"]; print ""; skipping = 1; seen_begin++; next }
  $0 == end { skipping = 0; seen_end++ }
  !skipping { print }
  END { if (seen_begin != 1 || seen_end != 1) exit 3 }
' "$ledger") || {
  printf 'render-sqlite-feature-matrix: %s needs exactly one %s and one %s line\n' "$ledger" "$begin" "$end" >&2
  exit 1
}

if ((check)); then
  if ! diff -u "$ledger" <(printf '%s\n' "$rendered") >&2; then
    printf 'render-sqlite-feature-matrix: %s is out of date with %s; run scripts/parity/render-sqlite-feature-matrix.sh\n' \
      "${ledger#"$repo_root"/}" "${matrix#"$repo_root"/}" >&2
    exit 1
  fi
  exit 0
fi
printf '%s\n' "$rendered" >"$ledger"
