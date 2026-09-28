#!/usr/bin/env bash
# The nextest runs of the kernel-failpoints stage (ops/ci/fast.sh), one per
# line as "<features><TAB><filter expression>":
#
#   - with --features failpoints: the lib's failpoints:: tests and every test
#     binary whose file is gated as a whole with
#     #![cfg(feature = "failpoints")];
#   - one run per test file that gates single tests with
#     #[cfg(feature = "failpoints")], with failpoints plus every feature the
#     file is gated on as a whole (wal_pipeline.rs needs wal_pipeline).
#     The kernel stage runs without failpoints, so these tests run only here.
#
# crates/bench/tests/ci_kernel_failpoints.rs checks that every
# failpoint-gated test file is planned with the features it needs.
#
#   ops/ci/kernel-failpoint-plan.sh [repository-root]
set -euo pipefail
root=$(cd "${1:-$(dirname "${BASH_SOURCE[0]}")/../..}" && pwd)

whole="kind(lib) & test(/^failpoints::/)"
per_test=()
for file in "$root"/crates/kernel/tests/*.rs; do
    name=$(basename "$file" .rs)
    if grep -qx '#!\[cfg(feature = "failpoints")\]' "$file"; then
        whole+=" | binary($name)"
    elif grep -q 'cfg(feature = "failpoints")' "$file"; then
        features=failpoints
        while read -r feature; do
            [[ -n $feature ]] && features+=",$feature"
        done < <(sed -n 's/^#!\[cfg(feature = "\([A-Za-z0-9_-]*\)")\]$/\1/p' "$file")
        per_test+=("$features"$'\t'"binary($name)")
    fi
done
[[ $whole == *"binary("* ]] || {
    printf 'kernel-failpoint-plan: no failpoint test binaries under crates/kernel/tests\n' >&2
    exit 1
}
printf 'failpoints\t%s\n' "$whole"
for line in ${per_test[@]+"${per_test[@]}"}; do
    printf '%s\n' "$line"
done
