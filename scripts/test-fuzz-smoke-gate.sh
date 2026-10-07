#!/usr/bin/env bash
# Execute the real preflight dispatcher with isolated command fixtures.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
fast_sh=${1:-$root/ops/ci/fast.sh}
scratch_parent=${TMPDIR:-$root/target/rt-scratch}
mkdir -p "$scratch_parent"
scratch=$(mktemp -d "$scratch_parent/redline-fuzz-gate.XXXXXX")
scratch=$(cd "$scratch" && pwd)
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/ops/ci" "$scratch/bin" "$scratch/target/debug"
cp "$fast_sh" "$scratch/ops/ci/fast.sh"
cp "$root/ops/ci/fuzz-smoke.sh" "$scratch/ops/ci/fuzz-smoke.sh"
mkdir -p "$scratch/docs/audits"
cp -R "$root/docs/audits/v5.1.1-post-release-evidence" "$scratch/docs/audits/"
printf 'set -euo pipefail\n' > "$scratch/ops/ci/lib.sh"
export FUZZ_GATE_REAL_BASH
FUZZ_GATE_REAL_BASH=$(command -v bash)
cat > "$scratch/bin/bash" <<'STUB'
#!/bin/sh
if [ "$1" = ops/ci/fuzz-smoke.sh ]; then
    exec "$FUZZ_GATE_REAL_BASH" "$@"
fi
exit 0
STUB
cat > "$scratch/bin/cargo" <<'STUB'
#!/bin/sh
case "$*" in
    'test --manifest-path fuzz/Cargo.toml --lib')
        printf 'library\n' >> "$FUZZ_GATE_CALLS"
        exit "$FUZZ_GATE_LIB_EXIT"
        ;;
    'check --manifest-path fuzz/Cargo.toml --bins')
        printf 'binaries\n' >> "$FUZZ_GATE_CALLS"
        exit "$FUZZ_GATE_BINS_EXIT"
        ;;
    'run --locked -q -p redlinedb --example readme') printf 'hello\n' ;;
esac
exit 0
STUB
printf '#!/bin/sh\nprintf "1\\n"\n' > "$scratch/target/debug/redlinedb"
chmod +x "$scratch/bin/bash" "$scratch/bin/cargo" "$scratch/target/debug/redlinedb"
cases=0
for scenario in success library-failure binaries-failure; do
    lib_exit=0; bins_exit=0; expected_exit=0
    case "$scenario" in
        library-failure) lib_exit=67; expected_exit=67 ;;
        binaries-failure) bins_exit=68; expected_exit=68 ;;
    esac
    calls="$scratch/$scenario.calls"
    : > "$calls"
    printf 'library\n' > "$scratch/expected"
    if [ "$scenario" != library-failure ]; then
        printf 'binaries\n' >> "$scratch/expected"
    fi
    result=0
    (
        cd "$scratch"
        PATH="$scratch/bin:$PATH" CI_FAST_STAGE=preflight \
        TMPDIR="$scratch" \
        CARGO_TARGET_DIR="$scratch/target" \
        REDLINEDB_BENCH_GIT_SHA=0000000000000000000000000000000000000000 \
        FUZZ_GATE_CALLS="$calls" FUZZ_GATE_LIB_EXIT="$lib_exit" \
        FUZZ_GATE_BINS_EXIT="$bins_exit" \
            "$FUZZ_GATE_REAL_BASH" ops/ci/fast.sh
    ) > "$scratch/$scenario.log" 2>&1 || result=$?
    if [ "$result" -ne "$expected_exit" ]; then
        cat "$scratch/$scenario.log" >&2
        printf '%s: preflight exit %s, expected %s\n' "$scenario" "$result" "$expected_exit" >&2
        exit 1
    fi
    diff -u "$scratch/expected" "$calls"
    cases=$((cases + 1))
done
printf 'Fuzz preflight dispatch: %s passed, 0 failed, 0 skipped.\n' "$cases"
