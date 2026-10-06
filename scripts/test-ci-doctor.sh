#!/usr/bin/env bash
# Run scripts/ci-doctor.sh against stand-in tools on a PATH that holds nothing
# else, so each profile's verdict depends only on the tools it names.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/base" "$work/browsers/chromium-1208"
for tool in bash env dirname grep head sed awk uname ls sha256sum cat tr; do
  ln -s "$(command -v "$tool")" "$work/base/$tool"
done
# tool <dir> <name> <stdout>: a stand-in that prints <stdout> and succeeds.
tool() {
  mkdir -p "$1"
  printf '#!/bin/sh\nprintf "%%s\\n" "%s"\n' "$3" > "$1/$2"
  chmod +x "$1/$2"
}
core=$work/core
tool "$core" rustc 'rustc 1.95.0 (59807616e 2026-04-14)'
tool "$core" cc ''
tool "$core" pkg-config ''
cat > "$core/cargo" <<'BIN'
#!/bin/sh
case "$1" in
  nextest) echo 'cargo-nextest 0.9.133' ;;
  audit) echo 'cargo-audit 0.22.1' ;;
esac
BIN
chmod +x "$core/cargo"
contributor=$work/contributor
for name in just git jq curl; do tool "$contributor" "$name" ''; done
tool "$contributor" node 'v22.12.0'
tool "$contributor" npm '10.9.0'
required=$work/required
tool "$required" cargo-deny 'cargo-deny 0.19.8'
tool "$required" gitleaks '8.21.2'

# doctor <label> <expected exit> <PATH dirs> [VAR=value...] -- [doctor args]:
# run the doctor; its output is left in $work/<label>.log.
doctor() {
  local label=$1 expected=$2 path=$3 status=0
  shift 3
  local vars=()
  while [[ $# -gt 0 && $1 != -- ]]; do vars+=("$1"); shift; done
  shift
  env -i PATH="$path:$work/base" PLAYWRIGHT_BROWSERS_PATH="$work/browsers" ${vars[@]+"${vars[@]}"} \
    bash "$root/scripts/ci-doctor.sh" "$@" > "$work/$label.log" 2>&1 || status=$?
  [[ $status == "$expected" ]] || fail "$label: exit $status, expected $expected: $(tail -n 3 "$work/$label.log")"
}
expect_line() { grep -Eq "$2" "$work/$1.log" || fail "$1: no line matching '$2' in: $(cat "$work/$1.log")"; }

doctor core-ok 0 "$core" -- --profile core
expect_line core-ok '^PASS rustc +1\.95\.0'
grep -q mold "$work/core-ok.log" && fail 'core-ok: the doctor still checks mold, which the build no longer uses'
rm "$core/pkg-config"
doctor core-no-pkg-config 1 "$core" -- --profile core
expect_line core-no-pkg-config '^FAIL pkg-config'
tool "$core" pkg-config ''
tool "$work/old-rust" rustc 'rustc 1.94.0 (00000000 2026-01-01)'
doctor core-old-rust 1 "$work/old-rust:$core" -- --profile core
expect_line core-old-rust '^FAIL rustc +expected=1\.95\.0 actual=1\.94\.0'
# rtk is optional for contributors.
doctor contributor-ok 0 "$contributor:$core" -- --profile contributor
expect_line contributor-ok '^PASS cargo-nextest +0\.9\.133'
expect_line contributor-ok '^PASS rtk +absent'
expect_line contributor-ok '^PASS node +v22\.12\.0'
rm "$contributor/node"
doctor contributor-no-node 1 "$contributor:$core" -- --profile contributor
expect_line contributor-no-node '^FAIL node +missing'
tool "$contributor" node 'v20.18.0'
doctor contributor-old-node 1 "$contributor:$core" -- --profile contributor
expect_line contributor-old-node '^FAIL node +expected=22\.x actual=v20\.18\.0'
tool "$contributor" node 'v22.12.0'
doctor unknown-profile 64 "$core" -- --profile everything
# The required profile stops before checking anything on other platforms.
printf '#!/bin/sh\ncase "$1" in -s) echo Darwin ;; -m) echo arm64 ;; esac\n' > "$work/darwin-uname"
mkdir -p "$work/darwin" && mv "$work/darwin-uname" "$work/darwin/uname" && chmod +x "$work/darwin/uname"
doctor required-darwin 2 "$work/darwin:$required:$contributor:$core" -- --profile required
expect_line required-darwin 'runs only on Linux x86_64'
if [[ $(uname -s)/$(uname -m) == Linux/x86_64 ]]; then
  # On Linux x86_64 every required tool is checked; the stand-in jankurai does
  # not carry the pinned digest, so the profile fails on it alone.
  printf '#!/bin/sh\necho jankurai 1.6.11\n' > "$work/jankurai"
  chmod +x "$work/jankurai"
  doctor required-linux 1 "$required:$contributor:$core" REDLINE_TESTING_POSTGRES_URL=postgres://example REDLINE_JANKURAI_BIN="$work/jankurai" -- --profile required
  for check in node npm chromium postgres cargo-audit cargo-deny gitleaks ci-required; do
    expect_line required-linux "^PASS $check "
  done
  expect_line required-linux '^FAIL jankurai +sha256 [0-9a-f]{64} is not the pinned'
  tool "$required" node 'v20.18.0'
  doctor required-old-node 1 "$required:$contributor:$core" REDLINE_TESTING_POSTGRES_URL=postgres://example -- --profile required
  expect_line required-old-node '^FAIL node +expected=22\.x actual=v20\.18\.0'
fi

[[ $failures == 0 ]] || { printf '%d ci-doctor check(s) failed\n' "$failures" >&2; exit 1; }
printf 'ci-doctor profile tests passed.\n'
