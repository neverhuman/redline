#!/usr/bin/env bash
# Lint the SQLite parity ledger (docs/sqlite-parity.md) and its source,
# docs/sqlite-feature-matrix.json. Fails when:
#   - a row has an unknown status or proof kind, a duplicate id, or a missing field;
#   - a `pass` row lists an open subfeature;
#   - a row with tests says `none`, or a row with no tests claims any proof;
#   - a test `path::fn` names a file without that function, or an artifact path
#     or glob matches nothing;
#   - a corpus case id is not in the official sqlite_parity catalog;
#   - the rendered tables differ from the JSON
#     (scripts/parity/render-sqlite-feature-matrix.sh --check);
#   - the ledger still carries "2445/2445"-style whole-corpus pass prose;
#   - a rendered row breaks the older prose rules below.
# SQLITE_PARITY_LEDGER_ROOT points it at another tree (scripts/parity/test-lint-sqlite-parity-ledger.sh).
set -euo pipefail

repo_root=${SQLITE_PARITY_LEDGER_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}
cd "$repo_root"

ledger="docs/sqlite-parity.md"
matrix="docs/sqlite-feature-matrix.json"
corpus="subrepos/redline-testing/corpus/sqlite_parity"
failures=0
fail() {
    printf 'lint-sqlite-parity-ledger: %s\n' "$*" >&2
    failures=1
}

# Shape, vocabulary and status rules, from the JSON.
while IFS= read -r problem; do
    [[ -n $problem ]] && fail "$matrix: $problem"
done < <(jq -r '
  (.features // []) as $rows
  | ($rows | group_by(.id) | map(select(length > 1) | "duplicate id \(.[0].id)")[]),
    ($rows[] | . as $row
      | (["id","section","feature","status","proof_kind","tests","artifacts","corpus_case_ids","open_subfeatures","owner","notes"]
          | map(select($row[.] == null)) | map("\($row.id // "?"): missing \(.)")[]),
        (if (.status | IN("pass","partial","fail","not-started","rejects-by-design")) | not
          then "\(.id): unknown status \(.status)" else empty end),
        (if (.proof_kind | IN("semantic_match","readback_only","intentional_reject","known_divergence","none")) | not
          then "\(.id): unknown proof_kind \(.proof_kind)" else empty end),
        (if .status == "pass" and ((.open_subfeatures // []) | length) > 0
          then "\(.id): a pass row lists open subfeatures" else empty end),
        (if .proof_kind == "none" and ((.tests // []) | length) > 0
          then "\(.id): proof_kind none but tests are listed" else empty end),
        (if .proof_kind != "none" and ((.tests // []) | length) == 0
          then "\(.id): proof_kind \(.proof_kind) needs at least one test" else empty end),
        ((.tests // [])[] | select(test("^[^:]+\\.rs::[A-Za-z_][A-Za-z0-9_]*$") | not)
          | "\($row.id): test \(.) is not path.rs::fn"))
' "$matrix")

# Every test function exists in its file.
while IFS=$'\t' read -r id test; do
    [[ -n $id ]] || continue
    path=${test%%::*}
    fn=${test#*::}
    if [[ ! -f $path ]]; then
        fail "$matrix: $id: $path does not exist"
    elif ! grep -Eq "fn ${fn}[[:space:]]*[(<]" "$path"; then
        fail "$matrix: $id: $path has no fn $fn"
    fi
done < <(jq -r '.features[] | .id as $id | .tests[] | "\($id)\t\(.)"' "$matrix")

# Every artifact path or glob matches something tracked.
while IFS=$'\t' read -r id artifact; do
    [[ -n $id ]] || continue
    if [[ $artifact == *'**'* ]]; then
        prefix=${artifact%%\*\**}
        [[ -n $(git ls-files -- "${prefix%/}" | head -n 1) ]] || fail "$matrix: $id: $artifact matches no tracked file"
    elif [[ ! -e $artifact ]]; then
        fail "$matrix: $id: $artifact does not exist"
    fi
done < <(jq -r '.features[] | .id as $id | .artifacts[] | "\($id)\t\(.)"' "$matrix")

# Every corpus case id is in the official catalog (manifest ids are numbers).
catalog=$(jq -r '.[].id | tostring | if test("^[0-9]+$") and length < 5 then ("00000" + .)[-5:] else . end' \
    "$corpus/generated_manifest.json" "$corpus"/cases/*.json | sort -u)
while IFS=$'\t' read -r id case_id; do
    [[ -n $id ]] || continue
    grep -qxF "$case_id" <<<"$catalog" || fail "$matrix: $id: corpus case $case_id is not in $corpus"
done < <(jq -r '.features[] | .id as $id | .corpus_case_ids[] | "\($id)\t\(.)"' "$matrix")

# The tables are the JSON, rendered.
if ! bash scripts/parity/render-sqlite-feature-matrix.sh --check --matrix "$matrix" --ledger "$ledger" 2>/dev/null; then
    fail "$ledger: the feature tables differ from $matrix; run scripts/parity/render-sqlite-feature-matrix.sh"
fi

# No whole-corpus pass prose: counts come from generated evidence.
if grep -nE '2445 ?(/|of) ?2445' "$ledger" "$matrix" >&2; then
    fail "a whole-corpus pass count (2445/2445) is typed into the ledger; point at the generated report"
fi

# Older prose rules, on the rendered tables
# (| Feature row | Status | Proof | Tests | Owner | Notes |).
awk '
BEGIN {
    FS = "|"
}

function trim(value) {
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
    return value
}

/^\| Feature row \| Status \|/ {
    in_feature_table = 1
    next
}

in_feature_table && $0 !~ /^\|/ {
    in_feature_table = 0
}

!in_feature_table {
    next
}

/^\|---/ {
    next
}

{
    # An escaped pipe (\|) inside a cell is not a column separator.
    line = $0
    gsub(/\\\|/, "\001", line)
    split(line, cell, "|")
    feature = trim(cell[2])
    status = trim(cell[3])
    notes = trim(cell[7])
    lower_feature = tolower(feature)
    lower_status = tolower(status)
    lower_notes = tolower(notes)

    if (lower_status !~ /^(pass|partial|fail|not-started|rejects-by-design)$/) {
        printf "%s:%d: unknown SQLite parity status %s for %s\n", FILENAME, FNR, status, feature > "/dev/stderr"
        failures = 1
    }

    if (lower_status == "pass" && lower_notes ~ /(incomplete|not complete|remains partial|remain partial|followup|follow-up|still|not yet)/) {
        printf "%s:%d: pass row admits an incomplete/partial gap: %s\n", FILENAME, FNR, feature > "/dev/stderr"
        failures = 1
    }

    if (lower_status == "pass" && lower_feature ~ /pragma/ && (lower_feature ~ /rejected/ || lower_notes ~ /(reject|unsupported)/)) {
        printf "%s:%d: rejected PRAGMA row must not be a parity pass: %s\n", FILENAME, FNR, feature > "/dev/stderr"
        failures = 1
    }

    if (lower_feature ~ /pragma/ && lower_feature ~ /rejected/ && lower_status != "rejects-by-design") {
        printf "%s:%d: rejected PRAGMA row must use rejects-by-design: %s\n", FILENAME, FNR, feature > "/dev/stderr"
        failures = 1
    }
}

END {
    exit failures
}
' "$ledger" || failures=1

exit "$failures"
