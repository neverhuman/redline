#!/usr/bin/env bash
# Offline tests for ops/ci/ensure-rust.sh (CI-04). rustup is a stand-in that
# records every call, so no toolchain is downloaded:
#   - an installed toolchain with its components needs no install call, and
#     the checks run with RUSTUP_AUTO_INSTALL=0;
#   - a missing toolchain is installed with the components from
#     rust-toolchain.toml, retrying failed attempts;
#   - a missing component is added without reinstalling the toolchain;
#   - an install that keeps failing stops after the attempt limit;
#   - a toolchain that reports another version, a toolchain file that does
#     not pin an exact release, and a runner without rustup fail closed;
#   - `rustup default` and `rustup self` are never called.
#
# Usage: bash ops/ci/tests/ensure-rust.sh
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
script=$root/ops/ci/ensure-rust.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }

[[ -f $script ]] || { printf 'FAIL: %s does not exist\n' "$script" >&2; exit 1; }

toolchain=$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$root/rust-toolchain.toml")
[[ -n $toolchain ]] || { printf 'FAIL: cannot read the channel of rust-toolchain.toml\n' >&2; exit 1; }
triple=x86_64-unknown-linux-gnu

# The stand-in keeps its state in $STATE: `toolchain` (the installed one),
# `components` (rustup's --installed listing), `failures` (install calls
# that fail before one succeeds) and `reports` (the version rustc prints,
# default the installed one). Every call is logged with RUSTUP_AUTO_INSTALL.
mkdir -p "$work/bin"
cat > "$work/bin/rustup" <<'SHIM'
#!/usr/bin/env bash
set -u
printf '%s|auto=%s\n' "$*" "${RUSTUP_AUTO_INSTALL-unset}" >> "$STATE/log"
installed=$(cat "$STATE/toolchain" 2>/dev/null || true)
flaky() {
  local left
  left=$(cat "$STATE/failures" 2>/dev/null || echo 0)
  if (( left > 0 )); then
    echo $((left - 1)) > "$STATE/failures"
    echo "error: could not download file from 'https://static.rust-lang.org/': timed out" >&2
    exit 1
  fi
}
add_components() {
  local component
  for component in "$@"; do
    printf '%s-%s\n' "$component" "$TRIPLE" >> "$STATE/components"
  done
}
case "$1 ${2:-}" in
  "run "*)
    [[ $2 == "$installed" ]] || { echo "error: toolchain '$2-$TRIPLE' is not installed" >&2; exit 1; }
    printf 'rustc %s (0123abcd 2026-01-01)\n' "$(cat "$STATE/reports" 2>/dev/null || echo "$installed")"
    ;;
  "component list")
    [[ -n $installed ]] || { echo "error: toolchain is not installed" >&2; exit 1; }
    printf 'cargo-%s\nrust-std-%s\nrustc-%s\n' "$TRIPLE" "$TRIPLE" "$TRIPLE"
    cat "$STATE/components" 2>/dev/null || true
    ;;
  "toolchain install")
    flaky
    shift 2
    name=$1
    shift
    : > "$STATE/components"
    while (( $# )); do
      if [[ $1 == --component ]]; then IFS=, read -r -a wanted <<< "$2"; add_components "${wanted[@]}"; shift; fi
      shift
    done
    echo "$name" > "$STATE/toolchain"
    ;;
  "component add")
    flaky
    shift 2
    [[ $1 == --toolchain ]] && shift 2
    add_components "$@"
    ;;
  *) echo "rustup stand-in: unexpected call: $*" >&2; exit 1 ;;
esac
SHIM
chmod +x "$work/bin/rustup"

# run_case <label> [VAR=value...]: a fresh state from the variables
# toolchain=, components=, failures=, reports=; sets $status.
run_case() {
  local state=$work/$1
  shift
  mkdir -p "$state"
  local toolchain_file=$root/rust-toolchain.toml attempts=3 args=()
  for arg in "$@"; do
    case $arg in
      toolchain=*) printf '%s\n' "${arg#*=}" > "$state/toolchain" ;;
      components=*) tr ' ' '\n' <<< "${arg#*=}" | sed '/^$/d' | sed "s/\$/-$triple/" > "$state/components" ;;
      failures=*) printf '%s\n' "${arg#*=}" > "$state/failures" ;;
      reports=*) printf '%s\n' "${arg#*=}" > "$state/reports" ;;
      toolchain_file=*) toolchain_file=${arg#*=} ;;
      attempts=*) attempts=${arg#*=} ;;
      *) args+=("$arg") ;;
    esac
  done
  : > "$state/log"
  : > "$state/github_env"
  status=0
  env -u CARGO_INCREMENTAL PATH="$work/bin:$PATH" STATE="$state" TRIPLE=$triple GITHUB_ENV="$state/github_env" \
    CI_RUST_TOOLCHAIN_FILE="$toolchain_file" CI_ENSURE_RUST_ATTEMPTS="$attempts" CI_ENSURE_RUST_DELAY=0 \
    ${args[@]+"${args[@]}"} bash "$script" > "$state/out" 2>&1 || status=$?
}
calls() { grep -c "^$2" "$work/$1/log" || true; }
never_global() {
  if grep -Eq '^(default|self|override|set) ' "$work/$1/log"; then
    fail "$1: changed a global rustup setting: $(grep -E '^(default|self|override|set) ' "$work/$1/log" | head -n 1)"
  fi
}

# 1. Everything is installed: offline checks only.
run_case installed toolchain="$toolchain" components="rustfmt clippy"
[[ $status == 0 ]] || fail "installed: exit $status: $(tail -n 3 "$work/installed/out")"
[[ $(calls installed 'toolchain install') == 0 && $(calls installed 'component add') == 0 ]] \
  || fail "installed: tried to install although the toolchain and components were present: $(cat "$work/installed/log")"
grep -q "^run $toolchain rustc -V|auto=0$" "$work/installed/log" \
  || fail "installed: rustc was not checked offline with RUSTUP_AUTO_INSTALL=0: $(cat "$work/installed/log")"
grep -q "^component list --installed --toolchain $toolchain|auto=0$" "$work/installed/log" \
  || fail "installed: components were not listed offline: $(cat "$work/installed/log")"
grep -qx 'CARGO_INCREMENTAL=0' "$work/installed/github_env" || fail "installed: CARGO_INCREMENTAL=0 not exported"
never_global installed

# 2. An existing CARGO_INCREMENTAL is kept.
run_case keeps-incremental toolchain="$toolchain" components="rustfmt clippy" CARGO_INCREMENTAL=1
[[ $status == 0 ]] || fail "keeps-incremental: exit $status"
! grep -q '^CARGO_INCREMENTAL=' "$work/keeps-incremental/github_env" \
  || fail "keeps-incremental: overrode the job's CARGO_INCREMENTAL"

# 3. The toolchain is missing and the first two downloads fail.
run_case flaky-install failures=2
[[ $status == 0 ]] || fail "flaky-install: exit $status: $(tail -n 3 "$work/flaky-install/out")"
[[ $(calls flaky-install 'toolchain install') == 3 ]] \
  || fail "flaky-install: expected 3 install attempts, got $(calls flaky-install 'toolchain install')"
grep -q "^toolchain install $toolchain --profile minimal --no-self-update --component rustfmt,clippy|" "$work/flaky-install/log" \
  || fail "flaky-install: unexpected install call: $(grep '^toolchain install' "$work/flaky-install/log" | head -n 1)"
never_global flaky-install

# 4. The toolchain is present without clippy: only clippy is added.
run_case missing-component toolchain="$toolchain" components=rustfmt
[[ $status == 0 ]] || fail "missing-component: exit $status: $(tail -n 3 "$work/missing-component/out")"
[[ $(calls missing-component 'toolchain install') == 0 ]] || fail "missing-component: reinstalled the toolchain"
grep -q "^component add --toolchain $toolchain clippy|" "$work/missing-component/log" \
  || fail "missing-component: clippy was not added: $(cat "$work/missing-component/log")"
never_global missing-component

# 5. Every download fails: stop after the attempt limit.
run_case unreachable failures=99 attempts=3
[[ $status != 0 ]] || fail "unreachable: succeeded without a toolchain"
[[ $(calls unreachable 'toolchain install') == 3 ]] \
  || fail "unreachable: expected exactly 3 attempts, got $(calls unreachable 'toolchain install')"
grep -q 'after 3 attempts' "$work/unreachable/out" || fail "unreachable: no attempt count in: $(tail -n 2 "$work/unreachable/out")"

# 6. The installed toolchain reports another version.
run_case wrong-version toolchain="$toolchain" components="rustfmt clippy" reports=1.0.0
[[ $status != 0 ]] || fail "wrong-version: accepted a toolchain that reports rustc 1.0.0"

# 7. A toolchain file that does not pin an exact release.
printf '[toolchain]\nchannel = "stable"\ncomponents = ["rustfmt"]\n' > "$work/stable.toml"
run_case floating toolchain_file="$work/stable.toml"
[[ $status != 0 ]] || fail "floating: accepted channel = \"stable\""
grep -q 'exact release' "$work/floating/out" || fail "floating: unexpected message: $(tail -n 1 "$work/floating/out")"
[[ ! -s $work/floating/log ]] || fail "floating: called rustup: $(head -n 1 "$work/floating/log")"

# 8. No rustup anywhere (only when this host has none outside PATH either).
if ! PATH=/usr/bin:/bin command -v rustup >/dev/null 2>&1; then
  mkdir -p "$work/empty-home"
  status=0
  env PATH=/usr/bin:/bin HOME="$work/empty-home" CARGO_HOME="$work/empty-home/.cargo" \
    CI_ENSURE_RUST_DELAY=0 bash "$script" > "$work/no-rustup.out" 2>&1 || status=$?
  [[ $status != 0 ]] || fail "no-rustup: succeeded without rustup"
  grep -q 'rustup' "$work/no-rustup.out" || fail "no-rustup: the failure does not name rustup: $(cat "$work/no-rustup.out")"
fi

[[ $failures == 0 ]] || { printf '%d ensure-rust check(s) failed\n' "$failures" >&2; exit 1; }
printf 'ensure-rust.sh tests passed.\n'
