#!/usr/bin/env bash
# The report-only audit must leave the source audit files untouched.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
mkdir -p target/review
fixture="$(mktemp -d target/review/report-audit-test.XXXXXX)"
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/bin"
cat > "$fixture/bin/jankurai" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
json='' md='' no_history=false
printf '%s\n' "$*" > "$REDLINE_REPORT_AUDIT_PROBE_LOG"
while (($#)); do
  case "$1" in
    --json) json="${2:?}"; shift 2 ;;
    --md) md="${2:?}"; shift 2 ;;
    --no-score-history) no_history=true; shift ;;
    --score-history|--score-history-csv) printf 'source score history requested\n' >&2; exit 1 ;;
    *) shift ;;
  esac
done
[[ $no_history == true && -n $json && -n $md ]] || {
  printf 'report audit lacks scratch outputs or --no-score-history\n' >&2
  exit 1
}
for output in "$json" "$md"; do
  case "$output" in
    target/*) ;;
    *) printf 'report audit would write source path: %s\n' "$output" >&2; exit 1 ;;
  esac
done
printf '{"score":1}\n' > "$json"
printf '# scratch score\n' > "$md"
STUB
chmod +x "$fixture/bin/jankurai"

before="$(git status --porcelain -- .jankurai)"
export REDLINE_REPORT_AUDIT_PROBE_LOG="$fixture/arguments"
output="$(PATH="$PWD/$fixture/bin:$PATH" bash scripts/just/sqlite-report-audit.sh "$fixture/output")"
[[ $output == "$fixture/output/repo-score.json" && -s $output ]] || {
  printf 'report audit did not return its scratch JSON\n' >&2
  exit 1
}
[[ -s "$fixture/output/repo-score.md" ]] || exit 1
[[ $(git status --porcelain -- .jankurai) == "$before" ]] || {
  printf 'report audit changed .jankurai/\n' >&2
  exit 1
}

# A target/ symlink or traversal must not redirect the audit into .jankurai/.
ln -s "$repo_root/.jankurai" "$fixture/source-link"
if PATH="$PWD/$fixture/bin:$PATH" bash scripts/just/sqlite-report-audit.sh "$fixture/source-link" >/dev/null 2>&1; then
  printf 'report audit accepted a symlink to .jankurai/\n' >&2
  exit 1
fi
if PATH="$PWD/$fixture/bin:$PATH" bash scripts/just/sqlite-report-audit.sh target/../.jankurai >/dev/null 2>&1; then
  printf 'report audit accepted a path traversal to .jankurai/\n' >&2
  exit 1
fi
linked_root="$fixture/linked-root"
mkdir -p "$linked_root/scripts/just" "$linked_root/.jankurai"
cp scripts/just/sqlite-report-audit.sh "$linked_root/scripts/just/"
ln -s .jankurai "$linked_root/target"
if bash "$linked_root/scripts/just/sqlite-report-audit.sh" >/dev/null 2>&1; then
  printf 'report audit accepted a target/ symlink to .jankurai/\n' >&2
  exit 1
fi
[[ ! -e "$linked_root/.jankurai/sqlite-parity-report-audit" ]] || {
  printf 'report audit created a directory through the target/ symlink\n' >&2
  exit 1
}

update_case="$(sed -n '/^  sqlite-parity-report-update)/,/^    ;;/p' scripts/just/run.sh)"
[[ $update_case == *'redlinedb_report_score_json="$(bash scripts/just/sqlite-report-audit.sh)"'* \
   && $update_case != *'"$0" score'* ]] || {
  printf 'report update is not routed through the scratch audit\n' >&2
  exit 1
}
printf 'sqlite parity report audit writes only target/ scratch files\n'
