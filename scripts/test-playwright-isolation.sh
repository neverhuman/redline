#!/usr/bin/env bash
# Exercise the real integration dispatcher without building or downloading.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
dispatcher=${1:-$repo/scripts/ci-family.sh}
scratch=${PLAYWRIGHT_TEST_SCRATCH_ROOT:-$repo/target/review}
mkdir -p "$scratch"
fixture=$(mktemp -d "$scratch/playwright-isolation.XXXXXX")
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/bin" "$fixture/home/.cache/ms-playwright/__dirlock"
printf '%s\n' unrelated-installer > "$fixture/home/.cache/ms-playwright/__dirlock/owner"
export PLAYWRIGHT_TEST_HOME="$fixture/home"
export PATH="$fixture/bin:$PATH"
export INSTALL_LOG="$fixture/calls"
# These fixtures exercise checkout-local defaults; external Cargo targets have
# separate dispatcher controls in test-ci-target-directory.test.mjs.
unset PLAYWRIGHT_BROWSERS_PATH FAIL_INSTALL FAIL_TEST CARGO_TARGET_DIR
cat > "$fixture/bin/npx" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
cache=${PLAYWRIGHT_BROWSERS_PATH:-$PLAYWRIGHT_TEST_HOME/.cache/ms-playwright}
case "$*" in
  '--no-install playwright install chromium')
    test "${PWD##*/}" = web
    if [ -d "$cache/__dirlock" ]; then exit 77; fi
    if [ -n "${FAIL_INSTALL:-}" ]; then exit "$FAIL_INSTALL"; fi
    mkdir -p "$cache"
    touch "$cache/chromium-installed"
    printf 'install %s\n' "$cache" >> "$INSTALL_LOG"
    ;;
  '--no-install playwright test')
    test -f "$cache/chromium-installed"
    test "$REDLINE_WEB_TARGET_BIN" = "$EXPECTED_ROOT/target/release/redlinedb"
    printf 'test %s\n' "$cache" >> "$INSTALL_LOG"
    if [ -n "${FAIL_TEST:-}" ]; then exit "$FAIL_TEST"; fi
    ;;
  *) printf 'unexpected npx arguments: %s\n' "$*" >&2; exit 78 ;;
esac
STUB
chmod +x "$fixture/bin/npx"
make_checkout() {
    local checkout=$1
    mkdir -p "$checkout/scripts" "$checkout/subrepos/redline-web/apps/web"
    cp "$dispatcher" "$checkout/scripts/ci-family.sh"
    printf '#!/usr/bin/env bash\nset -euo pipefail\ntest "$*" = --all\n' > "$checkout/scripts/build-from-source.sh"
    printf '#!/usr/bin/env bash\nset -euo pipefail\ntest "$1" = "$EXPECTED_ROOT/target/release"\n' > "$checkout/scripts/test-binaries.sh"
    chmod +x "$checkout/scripts/build-from-source.sh"
}
run_checkout() {
    export EXPECTED_ROOT=$1
    bash "$1/scripts/ci-family.sh" integration
}
first="$fixture/checkout one"
second="$fixture/checkout two"
make_checkout "$first"
make_checkout "$second"
run_checkout "$first"
run_checkout "$second"
test -f "$first/target/playwright-browsers/chromium-installed"
test -f "$second/target/playwright-browsers/chromium-installed"
test "$(sed -n '1p' "$INSTALL_LOG")" = "install $first/target/playwright-browsers"
test "$(sed -n '2p' "$INSTALL_LOG")" = "test $first/target/playwright-browsers"
test "$(sed -n '3p' "$INSTALL_LOG")" = "install $second/target/playwright-browsers"
test "$(sed -n '4p' "$INSTALL_LOG")" = "test $second/target/playwright-browsers"
run_checkout "$first"
PLAYWRIGHT_BROWSERS_PATH='' run_checkout "$second"
export PLAYWRIGHT_BROWSERS_PATH="$fixture/explicit cache"
run_checkout "$first"
test "$(tail -2 "$INSTALL_LOG" | head -1)" = "install $PLAYWRIGHT_BROWSERS_PATH"
test "$(tail -1 "$INSTALL_LOG")" = "test $PLAYWRIGHT_BROWSERS_PATH"
unset PLAYWRIGHT_BROWSERS_PATH
# A failed download must preserve its exit status and must not start the tests.
before=$(wc -l < "$INSTALL_LOG")
rc=0
FAIL_INSTALL=43 run_checkout "$first" || rc=$?
test "$rc" = 43
test "$(wc -l < "$INSTALL_LOG")" = "$before"
rc=0
FAIL_TEST=44 run_checkout "$first" || rc=$?
test "$rc" = 44
test -d "$PLAYWRIGHT_TEST_HOME/.cache/ms-playwright/__dirlock"
test "$(cat "$PLAYWRIGHT_TEST_HOME/.cache/ms-playwright/__dirlock/owner")" = unrelated-installer
printf '%s\n' 'PASS: separate checkout caches, reuse, empty/default env, explicit override, install/test exit propagation, unrelated lock preserved (7 cases; 0 skips)'
