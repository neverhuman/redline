#!/usr/bin/env bash
# Keep the private security route in SECURITY.md and the issue chooser live
# and consistent:
#   - SECURITY.md sends reports to the canonical repository's private advisory
#     form, publishes no email address and never names a legacy repository;
#   - it keeps the supported-versions table and the response targets;
#   - .github/ISSUE_TEMPLATE/config.yml links the same advisory form.
#
#   scripts/check-security-policy.sh [repository-root]
set -euo pipefail
root=${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}
advisory=https://github.com/neverhuman/redline/security/advisories/new
policy=$root/SECURITY.md chooser=$root/.github/ISSUE_TEMPLATE/config.yml
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }
if [[ ! -f $policy ]]; then
  fail "SECURITY.md is missing"
else
  grep -qF "$advisory" "$policy" || fail "SECURITY.md does not link $advisory"
  ! grep -qE '[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+' "$policy" || fail "SECURITY.md publishes an email address"
  ! grep -qiE 'neverhuman/redlinedb|neverhumanbot/' "$policy" || fail "SECURITY.md names a legacy repository"
  for required in '5.0.x' '4.x' 'prerelease' '3 business days' '10 business days' '90 days' 'Safe harbor'; do
    grep -qiF "$required" "$policy" || fail "SECURITY.md does not state: $required"
  done
fi
if [[ ! -f $chooser ]]; then
  fail ".github/ISSUE_TEMPLATE/config.yml is missing"
else
  grep -qE '^blank_issues_enabled:[[:space:]]*true[[:space:]]*$' "$chooser" || fail "config.yml does not set blank_issues_enabled: true"
  awk -v url="$advisory" '$1 == "url:" && $2 == url && NF == 2 { found = 1 } END { exit !found }' "$chooser" ||
    fail "config.yml has no contact link to $advisory"
fi
[[ $failures == 0 ]] || { printf '%d security policy check(s) failed\n' "$failures" >&2; exit 1; }
printf 'Security policy route checks passed.\n'
