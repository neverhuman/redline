#!/usr/bin/env bash
# Tests for scripts/qualification/emit-sqlite-qualification.sh --from, on
# fixture logs in libtest's output format and a fixture probe receipt.
#
# Usage: bash scripts/qualification/test-emit-sqlite-qualification.sh
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
emit=$root/scripts/qualification/emit-sqlite-qualification.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }

logs=$work/logs
mkdir -p "$logs/abi-probe"
good() {
  cat >"$logs/rust-values.log" <<'EOF'
   Compiling redlinedb-sql v5.0.0
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.00s
     Running tests/sqlite_full_parity.rs (target/debug/deps/sqlite_full_parity-0123)

running 3 tests
test reference_build_metadata_is_available ... ok
test known_gap_pragmas_are_rejected_or_diverge ... ok
test slow_probe ... ignored, needs a large corpus

test result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 1.00s

EOF
  printf '0\n' >"$logs/rust-values.exit"
  printf 'running 1 test\nrusqlite_crate_version=0.37.0\nsqlite_version=3.50.2\ncompile_options:\n' >"$logs/sqlite-version.log"
  cat >"$logs/ffi.log" <<'EOF'
     Running unittests src/lib.rs (target/debug/deps/redlinedb-0123)

running 2 tests
test tests::a ... ok
test tests::b ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/blob_io.rs (target/debug/deps/blob_io-0123)

running 1 test
test open_read_write_close_round_trip ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests redlinedb

running 1 test
test crates/ffi/src/lib.rs - config (line 10) ... ignored

test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s

EOF
  printf '0\n' >"$logs/ffi.exit"
  printf '# header_sha256\thhh\n# probe_source_sha256\tppp\n# library\t/x/libredlinedb.so\tlll\n# control\t/x/libsqlite3.so\tccc\nengine\tlink\tcase\texit\tresult\nsqlite\tdynamic\tv3-zero\t0\tpass\nredline\tdynamic\tv3-zero\t0\tpass\nredline\tstatic\tv3-zero\t0\tpass\n' \
    >"$logs/abi-probe/receipt.tsv"
  printf '0\n' >"$logs/abi-probe.exit"
}
# emit_status <label> <want exit>
emit_status() {
  local got=0 output
  output=$(bash "$emit" --from "$logs" --out "$work/out" 2>&1) || got=$?
  [[ $got == "$2" ]] || fail "$1: exit $got, want $2: $output"
}
check() { # check <label> <file> <jq test>
  jq -e "$3" "$work/out/$2" >/dev/null || fail "$1: $2 fails $3: $(jq -c . "$work/out/$2" | head -c 600)"
}

good
emit_status 'every lane passes' 0
check 'rust values' sqlite_rust_values.json '.status == "passed" and .tests.passed == 2 and .tests.failed == 0 and .tests.ignored == 1'
check 'bundled version' sqlite_rust_values.json '.oracle.sqlite_version == "3.50.2" and .oracle.rusqlite == "0.37.0" and (.oracle.libsqlite3_sys | length > 0)'
check 'rust cases' sqlite_rust_values.json '.tests.cases[0] == {binary: "tests/sqlite_full_parity.rs", test: "reference_build_metadata_is_available", result: "ok"}'
check 'ffi counts' sqlite_c_abi.json '.ffi_tests.passed == 3 and .ffi_tests.ignored == 1 and .ffi_tests.status == "passed"'
check 'ffi binaries' sqlite_c_abi.json '[.ffi_tests.cases[].binary] | unique == ["doc-tests redlinedb", "src/lib.rs", "tests/blob_io.rs"]'
check 'doc-test names' sqlite_c_abi.json 'any(.ffi_tests.cases[]; .test == "crates/ffi/src/lib.rs - config (line 10)")'
check 'probe' sqlite_c_abi.json '.abi_probe.status == "passed" and .abi_probe.redline == {library_sha256: "lll", passed: 2, failed: 0} and .abi_probe.control.passed == 1 and .abi_probe.header_sha256 == "hhh"'
check 'overall' sqlite_c_abi.json '.status == "passed" and .source.commit != null'

good
sed -i 's/^test known_gap_pragmas_are_rejected_or_diverge ... ok/test known_gap_pragmas_are_rejected_or_diverge ... FAILED/; s/^test result: ok. 2 passed; 0 failed/test result: FAILED. 1 passed; 1 failed/' \
  "$logs/rust-values.log"
printf '101\n' >"$logs/rust-values.exit"
emit_status 'a failed rust test' 1
check 'failed rust test' sqlite_rust_values.json '.status == "failed" and .tests.failed == 1 and .tests.passed == 1'

good
printf 'running 1 test\n' >"$logs/sqlite-version.log"
emit_status 'no bundled version' 1
check 'no bundled version' sqlite_rust_values.json '.status == "error" and .oracle.sqlite_version == null'

good
sed -i 's/^test result: ok. 1 passed; 0 failed; 0 ignored/test result: ok. 2 passed; 0 failed; 0 ignored/' "$logs/ffi.log"
emit_status 'totals that disagree with the tests' 1
check 'totals disagree' sqlite_c_abi.json '.ffi_tests.status == "error"'

good
printf '   Compiling redlinedb-ffi\nerror[E0425]: cannot find value\n' >"$logs/ffi.log"
printf '101\n' >"$logs/ffi.exit"
emit_status 'ffi tests do not build' 1
check 'no ffi tests' sqlite_c_abi.json '.ffi_tests.status == "failed" and .ffi_tests.passed == 0 and .status == "failed"'

good
sed -i '/^# control/d; /^sqlite\t/d; s/^redline\tstatic\tv3-zero\t0\tpass/redline\tstatic\tv3-zero\t139\tsignal-11/' \
  "$logs/abi-probe/receipt.tsv"
printf '1\n' >"$logs/abi-probe.exit"
emit_status 'a probe case fails' 1
check 'probe failure' sqlite_c_abi.json '.abi_probe.status == "failed" and .abi_probe.redline.failed == 1 and .abi_probe.control.status == "skipped"'

good
rm "$logs/abi-probe/receipt.tsv" "$logs/abi-probe.exit"
emit_status 'the probe never ran' 1
check 'no probe' sqlite_c_abi.json '.abi_probe.status == "failed" and .abi_probe.exit_code == -1'

if ((failures)); then
  printf 'test-emit-sqlite-qualification: %d failure(s)\n' "$failures" >&2
  exit 1
fi
printf 'test-emit-sqlite-qualification: ok\n'
