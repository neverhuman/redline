#!/usr/bin/env bash
# Tests for scripts/check-launch-claims.sh on a throwaway git repository.
#
# Usage: bash scripts/test-launch-claims.sh
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
lint=$root/scripts/check-launch-claims.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }

repo=$work/repo
mkdir -p "$repo/docs/archive" "$repo/docs/migration" "$repo/docs/releases" "$repo/scripts"
git -C "$repo" init -q
cat >"$repo/README.md" <<'EOF'
# Fixture
RedlineDB is not a SQLite drop-in.
EOF
printf 'Older planning said: full SQLite compatibility (2445 cases).\n' >"$repo/docs/plan.md"
printf 'We are a drop-in.\n' >"$repo/docs/archive/old.md"
printf 'A 100%% SQLite drop-in.\n' >"$repo/docs/migration/old.md"
printf 'Nothing to see.\n' >"$repo/docs/releases/v1.md"
: >"$repo/scripts/launch-claims-allowlist.tsv"
git -C "$repo" add -A

# expect <label> <exit> [fragment]
expect() {
  local label=$1 want=$2 fragment=${3:-} got=0 output
  git -C "$repo" add -A
  output=$(bash "$lint" --root "$repo" 2>&1) || got=$?
  [[ $got == "$want" ]] || fail "$label: exit $got, want $want: $output"
  [[ -z $fragment || $output == *"$fragment"* ]] || fail "$label: output lacks '$fragment': $output"
}
row() { # row <kind> <line of --print output>
  local print path
  IFS=$'\t' read -r print path _ <<<"$2"
  printf '%s\t%s\t%s\treviewed in the fixture\n' "$print" "$path" "$1"
}

expect 'two unreviewed lines' 1 'README.md:2: unreviewed claim'
expect 'the docs hit is named' 1 'docs/plan.md:1: unreviewed claim'

output=$(bash "$lint" --root "$repo" --print)
[[ $(wc -l <<<"$output") == 2 ]] || fail "--print should list 2 hits (archive and migration excluded): $output"
{
  row qualified "$(grep README.md <<<"$output")"
  row historical "$(grep docs/plan.md <<<"$output")"
} >"$repo/scripts/launch-claims-allowlist.tsv"
expect 'every hit reviewed' 0 'ok (2 reviewed line(s))'

# A regenerated count keeps the review; new words do not.
printf 'Older planning said: full SQLite compatibility (2446 cases).\n' >"$repo/docs/plan.md"
expect 'a changed count' 0
printf 'Now: full SQLite compatibility (2446 cases).\n' >"$repo/docs/plan.md"
expect 'changed words' 1 'docs/plan.md:1: unreviewed claim'
expect 'the old row is stale' 1 'matches no line; remove it'

# The same words in another file need their own review.
printf 'Older planning said: full SQLite compatibility (2445 cases).\n' >"$repo/docs/plan.md"
cp "$repo/docs/plan.md" "$repo/docs/releases/v1.md"
expect 'a copy in docs/releases' 1 'docs/releases/v1.md:1: unreviewed claim'
printf 'Nothing to see.\n' >"$repo/docs/releases/v1.md"
expect 'back to reviewed' 0

# An untracked file is not published, so it is not checked.
printf 'faster than SQLite\n' >"$repo/docs/untracked.md"
output=$(bash "$lint" --root "$repo" 2>&1) || fail "an untracked file was checked: $output"
rm "$repo/docs/untracked.md"

# Case-insensitive, and each pattern is covered.
for claim in 'DROP-IN' '100% sqlite' '100% Safe-Rust' 'Faster Than SQLite'; do
  printf '%s\n' "$claim" >"$repo/docs/claim.md"
  expect "pattern $claim" 1 'docs/claim.md:1: unreviewed claim'
done
git -C "$repo" rm -q -f docs/claim.md

printf 'deadbeefdeadbeef\tdocs/plan.md\tmaybe\tno\n' >>"$repo/scripts/launch-claims-allowlist.tsv"
expect 'an unknown kind' 1 'use qualified or historical'

if ((failures)); then
  printf 'test-launch-claims: %d failure(s)\n' "$failures" >&2
  exit 1
fi
printf 'test-launch-claims: ok\n'
