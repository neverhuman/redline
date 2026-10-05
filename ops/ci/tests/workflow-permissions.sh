#!/usr/bin/env bash
# Tests for ops/ci/check-workflow-permissions.sh: a ci.yml, packages.yml or
# packages-cross.yml permission request beyond release-build.yml's acceptance
# grant is refused, in each YAML form, and requests within it pass.
#
# Usage: bash ops/ci/tests/workflow-permissions.sh
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
check=$root/ops/ci/check-workflow-permissions.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }

# fixture <label> <ci.yml jobs text>: release-build.yml grants contents and
# attestations read; packages.yml asks for contents read.
fixture() {
  local dir=$work/$1
  mkdir -p "$dir"
  cat > "$dir/release-build.yml" <<'YAML'
name: release-build
permissions:
  contents: read
jobs:
  validate:
    permissions: {contents: read}
  acceptance:
    needs: validate
    uses: ./.github/workflows/ci.yml
    permissions:
      contents: read   # the called workflow's ceiling
      attestations: read
  publish:
    permissions:
      contents: write
      id-token: write
YAML
  printf 'name: packages\npermissions:\n  contents: read\njobs:\n  build:\n    runs-on: ubuntu-24.04\n' > "$dir/packages.yml"
  printf 'name: packages-cross\npermissions:\n  contents: read\njobs:\n  build:\n    runs-on: ubuntu-24.04\n' > "$dir/packages-cross.yml"
  printf 'name: ci\njobs:\n%s\n' "$2" > "$dir/ci.yml"
}
expect_pass() {
  fixture "$1" "$2"
  bash "$check" "$work/$1" > "$work/$1.log" 2>&1 || fail "$1: refused: $(cat "$work/$1.log")"
}
expect_refusal() {
  fixture "$1" "$3"
  if bash "$check" "$work/$1" > "$work/$1.log" 2>&1; then
    fail "$1: accepted"
  elif ! grep -qF -- "$2" "$work/$1.log"; then
    fail "$1: expected '$2', got: $(cat "$work/$1.log")"
  fi
}

expect_pass within-grant '  a:
    permissions:
      contents: read
      # comment lines and blank lines are skipped

      attestations: read
    steps: []
  b:
    permissions: {contents: read, attestations: none}
  c:
    permissions: {}
  d:
    runs-on: ubuntu-24.04'
expect_refusal block-write 'ci.yml:8: job merge-report asks for pull-requests: write' '  fine:
    permissions: {contents: read}
  merge-report:
    permissions:
      contents: read
      pull-requests: write'
expect_refusal flow-write 'job lint asks for contents: write' '  lint:
    permissions: {attestations: read, contents: write}'
expect_refusal read-all 'job audit asks for *: read-all' '  audit:
    permissions: read-all'
expect_refusal workflow-level 'job - asks for actions: read' 'x: 1
permissions:
  actions: read
jobs:
  a:
    runs-on: ubuntu-24.04'
expect_refusal unknown-level 'job a asks for contents: maybe' '  a:
    permissions: {contents: maybe}'
expect_refusal nested-comment 'job a asks for contents: write' '  a:
    permissions:
      contents: write  # needed to push'

# packages.yml is held to the same grant.
fixture packages-write '  a:
    runs-on: ubuntu-24.04'
printf '  release:\n    permissions:\n      contents: write\n' >> "$work/packages-write/packages.yml"
if bash "$check" "$work/packages-write" > "$work/packages-write.log" 2>&1; then
  fail 'packages-write: accepted'
fi
grep -qF 'packages.yml:' "$work/packages-write.log" || fail "packages-write: $(cat "$work/packages-write.log")"

# packages-cross.yml is held to the same grant.
fixture packages-cross-write '  a:
    runs-on: ubuntu-24.04'
printf '  release:\n    permissions:\n      contents: write\n' >> "$work/packages-cross-write/packages-cross.yml"
if bash "$check" "$work/packages-cross-write" > "$work/packages-cross-write.log" 2>&1; then
  fail 'packages-cross-write: accepted'
fi
grep -qF 'packages-cross.yml:' "$work/packages-cross-write.log" || fail "packages-cross-write: $(cat "$work/packages-cross-write.log")"

# The grant is read from release-build.yml: widening it there widens the check.
fixture wider-grant '  a:
    permissions: {actions: read}'
awk '{ print } /^      attestations: read$/ { print "      actions: read" }' "$work/wider-grant/release-build.yml" > "$work/wider.yml"
mv "$work/wider.yml" "$work/wider-grant/release-build.yml"
bash "$check" "$work/wider-grant" > "$work/wider-grant.log" 2>&1 || fail "wider-grant: $(cat "$work/wider-grant.log")"

# This repository's workflows.
bash "$check" > "$work/self.log" 2>&1 || fail "this repository: $(cat "$work/self.log")"

[[ $failures == 0 ]] || { printf '%d workflow permission check(s) failed\n' "$failures" >&2; exit 1; }
printf 'Workflow permission tests passed.\n'
