#!/usr/bin/env bash
# Drive the real hook with a private non-Git fixture and stand-in tools.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
hook=${1:-$repo/ops/git-hooks/pre-push}
scratch=${PRE_PUSH_TEST_SCRATCH_ROOT:-$repo/target/review}
mkdir -p "$scratch"
fixture=$(mktemp -d "$scratch/pre-push-audit.XXXXXX")
trap 'rm -rf "$fixture"' EXIT
export PRE_PUSH_FIXTURE_ROOT="$fixture/checkout with spaces"
export PRE_PUSH_CALLS="$fixture/calls"
export PATH="$fixture/bin:$PATH"
mkdir -p "$fixture/bin" "$PRE_PUSH_FIXTURE_ROOT/scripts"
printf '#!/usr/bin/env bash\nexit 0\n' > "$PRE_PUSH_FIXTURE_ROOT/scripts/check_audit_policy_mirror.sh"
cat > "$PRE_PUSH_FIXTURE_ROOT/scripts/ci-local.sh" <<'CI'
#!/usr/bin/env bash
set -euo pipefail
test "$*" = pr-ci
printf 'ci-mirror\n' >> "$PRE_PUSH_CALLS"
exit "${CI_MIRROR_EXIT:-0}"
CI
cat > "$fixture/bin/git" <<'GIT'
#!/usr/bin/env bash
set -euo pipefail
test "$*" = 'rev-parse --show-toplevel'
printf '%s\n' "$PRE_PUSH_FIXTURE_ROOT"
GIT
cat > "$fixture/bin/jankurai" <<'AUDIT'
#!/usr/bin/env bash
set -euo pipefail
if [ -n "${AUDIT_EXIT:-}" ]; then exit "$AUDIT_EXIT"; fi
json= md= history=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --json) json=$2; shift ;;
    --md) md=$2; shift ;;
    --no-score-history) history=1 ;;
  esac
  shift
done
case "$json" in "$PRE_PUSH_FIXTURE_ROOT"/target/*) ;; *) exit 77 ;; esac
case "$md" in "$PRE_PUSH_FIXTURE_ROOT"/target/*) ;; *) exit 78 ;; esac
test "$history" = 1
test -d "$(dirname "$json")"
printf '{"hard_findings":0}\n' > "$json"
printf 'No findings.\n' > "$md"
printf 'audit-scratch\n' >> "$PRE_PUSH_CALLS"
AUDIT
chmod +x "$fixture/bin/git" "$fixture/bin/jankurai"
bash "$hook"
test "$(cat "$PRE_PUSH_CALLS")" = $'audit-scratch\nci-mirror'
test ! -e "$PRE_PUSH_FIXTURE_ROOT/.jankurai"
before=$(wc -l < "$PRE_PUSH_CALLS"); rc=0
AUDIT_EXIT=43 bash "$hook" >/dev/null 2>&1 || rc=$?
test "$rc" = 43
test "$(wc -l < "$PRE_PUSH_CALLS")" = "$before"
rc=0
CI_MIRROR_EXIT=44 bash "$hook" >/dev/null 2>&1 || rc=$?
test "$rc" = 44
test ! -e "$PRE_PUSH_FIXTURE_ROOT/.jankurai"
printf '%s\n' 'PASS: scratch audit/no history, audit failure, required-gate failure (3 cases; 0 skips)'
