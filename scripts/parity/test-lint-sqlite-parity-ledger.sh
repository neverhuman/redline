#!/usr/bin/env bash
# Tests for scripts/parity/lint-sqlite-parity-ledger.sh and
# scripts/parity/render-sqlite-feature-matrix.sh on a throwaway tree.
#
# Usage: bash scripts/parity/test-lint-sqlite-parity-ledger.sh
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }

tree=$work/tree
corpus=$tree/subrepos/redline-testing/corpus/sqlite_parity
mkdir -p "$tree/scripts/parity" "$tree/docs" "$tree/crates/demo/tests" "$corpus/cases"
cp "$root/scripts/parity/lint-sqlite-parity-ledger.sh" "$root/scripts/parity/render-sqlite-feature-matrix.sh" \
  "$tree/scripts/parity/"
cat >"$tree/crates/demo/tests/demo.rs" <<'EOF'
#[test]
fn alpha_matches() {}

#[test]
fn beta_rejects() {}
EOF
printf '[{"id": 1}, {"id": 2}]\n' >"$corpus/generated_manifest.json"
printf '[{"id": "10001"}]\n' >"$corpus/cases/a.json"
cat >"$tree/docs/sqlite-feature-matrix.json" <<'EOF'
{
  "schema": "redlinedb.sqlite-feature-matrix/v1",
  "features": [
    {"id": "demo.alpha", "section": "Demo", "feature": "Alpha `a|b`", "status": "pass",
     "proof_kind": "semantic_match", "tests": ["crates/demo/tests/demo.rs::alpha_matches"],
     "artifacts": ["crates/demo/**"], "corpus_case_ids": ["00001", "00002", "10001"],
     "open_subfeatures": [], "owner": "demo", "notes": "Alpha works."},
    {"id": "demo.beta", "section": "Demo", "feature": "Beta", "status": "partial",
     "proof_kind": "intentional_reject", "tests": ["crates/demo/tests/demo.rs::beta_rejects"],
     "artifacts": [], "corpus_case_ids": [], "open_subfeatures": ["gamma"], "owner": "demo",
     "notes": "Beta is refused."},
    {"id": "file.none", "section": "Files", "feature": "Nothing", "status": "not-started",
     "proof_kind": "none", "tests": [], "artifacts": [], "corpus_case_ids": [],
     "open_subfeatures": [], "owner": "demo", "notes": "No reader."}
  ]
}
EOF
printf '# Ledger\n\nIntro.\n\n<!-- sqlite-feature-matrix:begin -->\n<!-- sqlite-feature-matrix:end -->\n\n## After\n' \
  >"$tree/docs/sqlite-parity.md"
git -C "$tree" init -q
bash "$tree/scripts/parity/render-sqlite-feature-matrix.sh"
git -C "$tree" add -A
cp "$tree/docs/sqlite-feature-matrix.json" "$work/matrix.good"
cp "$tree/docs/sqlite-parity.md" "$work/ledger.good"

# expect <label> <exit> [fragment]
expect() {
  local label=$1 want=$2 fragment=${3:-} got=0 output
  output=$(SQLITE_PARITY_LEDGER_ROOT=$tree bash "$tree/scripts/parity/lint-sqlite-parity-ledger.sh" 2>&1) || got=$?
  [[ $got == "$want" ]] || fail "$label: exit $got, want $want: $output"
  [[ -z $fragment || $output == *"$fragment"* ]] || fail "$label: output lacks '$fragment': $output"
}
reset() { cp "$work/matrix.good" "$tree/docs/sqlite-feature-matrix.json"; cp "$work/ledger.good" "$tree/docs/sqlite-parity.md"; }
# edit <jq filter>: change the matrix and render it, so only the rule under test fails.
edit() {
  jq "$1" "$work/matrix.good" >"$tree/docs/sqlite-feature-matrix.json"
  bash "$tree/scripts/parity/render-sqlite-feature-matrix.sh"
}

grep -qF '| Alpha `a\|b` | pass | semantic_match | `crates/demo/tests/demo.rs`: `alpha_matches`; `crates/demo/**` | demo | Alpha works. Corpus cases: 00001–00002, 10001. |' \
  "$tree/docs/sqlite-parity.md" || fail "rendered row: $(grep Alpha "$tree/docs/sqlite-parity.md")"
grep -qF '| Beta | partial | intentional_reject |' "$tree/docs/sqlite-parity.md" || fail 'beta row not rendered'
grep -qF 'Open: gamma.' "$tree/docs/sqlite-parity.md" || fail 'open subfeatures not rendered'
grep -qx '## Files' "$tree/docs/sqlite-parity.md" || fail 'second section not rendered'
grep -qx '## After' "$tree/docs/sqlite-parity.md" || fail 'text after the block was lost'
expect 'the fixture' 0

edit '.features[0].tests = ["crates/demo/tests/demo.rs::gamma_matches"]'
expect 'a missing test function' 1 'has no fn gamma_matches'
edit '.features[0].tests = ["crates/demo/tests/nope.rs::alpha_matches"]'
expect 'a missing test file' 1 'nope.rs does not exist'
edit '.features[0].tests = ["crates/demo/tests/demo.rs"]'
expect 'a test without ::fn' 1 'is not path.rs::fn'
edit '.features[0].corpus_case_ids = ["00003"]'
expect 'an unknown corpus case' 1 'corpus case 00003 is not in'
edit '.features[0].open_subfeatures = ["delta"]'
expect 'a pass row with an open subfeature' 1 'a pass row lists open subfeatures'
edit '.features[1].proof_kind = "vibes"'
expect 'an unknown proof kind' 1 'unknown proof_kind vibes'
edit '.features[1].status = "done"'
expect 'an unknown status' 1 'unknown status done'
edit '.features[2].tests = ["crates/demo/tests/demo.rs::alpha_matches"]'
expect 'proof none with tests' 1 'proof_kind none but tests are listed'
edit '.features[1].tests = []'
expect 'a proof kind without tests' 1 'needs at least one test'
edit '.features[0].artifacts = ["crates/missing/**"]'
expect 'an artifact glob matching nothing' 1 'matches no tracked file'
edit '.features[1].id = "demo.alpha"'
expect 'a duplicate id' 1 'duplicate id demo.alpha'
edit 'del(.features[1].owner)'
expect 'a missing field' 1 'missing owner'

reset
sed -i 's/Alpha works\./Alpha works well./' "$tree/docs/sqlite-parity.md"
expect 'a hand edit of the tables' 1 'the feature tables differ'
reset
printf 'Every case passes: 2445/2445.\n' >>"$tree/docs/sqlite-parity.md"
expect 'typed whole-corpus prose' 1 '2445/2445'
reset
jq '.features[0].notes = "Alpha is still missing a piece."' "$work/matrix.good" >"$tree/docs/sqlite-feature-matrix.json"
bash "$tree/scripts/parity/render-sqlite-feature-matrix.sh"
expect 'a pass row that admits a gap' 1 'pass row admits an incomplete/partial gap'

reset
output=$(bash "$tree/scripts/parity/render-sqlite-feature-matrix.sh" --check --matrix "$tree/docs/sqlite-feature-matrix.json" \
  --ledger "$tree/docs/sqlite-parity.md" 2>&1) || fail "render --check on a rendered ledger: $output"
printf '<!-- sqlite-feature-matrix:end -->\n' >>"$tree/docs/sqlite-parity.md"
bash "$tree/scripts/parity/render-sqlite-feature-matrix.sh" --matrix "$tree/docs/sqlite-feature-matrix.json" \
  --ledger "$tree/docs/sqlite-parity.md" 2>/dev/null && fail 'render accepted two end markers'

if ((failures)); then
  printf 'test-lint-sqlite-parity-ledger: %d failure(s)\n' "$failures" >&2
  exit 1
fi
printf 'test-lint-sqlite-parity-ledger: ok\n'
