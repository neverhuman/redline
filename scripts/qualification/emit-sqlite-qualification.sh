#!/usr/bin/env bash
# Emit the two SQLite qualification objects (SQ-08) from test runs, never by hand:
#   <out>/sqlite_rust_values.json  typed values compared in-process with the SQLite
#                                  library rusqlite bundles
#                                  (cargo test -p redlinedb-sql --test sqlite_full_parity)
#   <out>/sqlite_c_abi.json        the C ABI: cargo test -p redlinedb-ffi, and the
#                                  upstream-header probe scripts/compatibility/phase2-abi-probe.sh
# Neither is the official SQL/CLI corpus (redline-testing) and neither says
# anything about the SQLite file format.
#
#   bash scripts/qualification/emit-sqlite-qualification.sh [--out DIR]
#       runs the tests (default DIR: target/qualification)
#   bash scripts/qualification/emit-sqlite-qualification.sh --from DIR [--out DIR]
#       builds the JSON from logs already in DIR: rust-values.log, rust-values.exit,
#       sqlite-version.log, ffi.log, ffi.exit, abi-probe/receipt.tsv, abi-probe.exit
#
# Exit status: 0 when every lane ran and passed; 1 when a test or probe case
# failed or a lane could not run. The JSON is written either way.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
out=$root/target/qualification
from=''
while [[ $# -gt 0 ]]; do
  case $1 in
    --out) out=$2; shift 2 ;;
    --from) from=$2; shift 2 ;;
    *) sed -n '2,20p' "${BASH_SOURCE[0]}" >&2; exit 64 ;;
  esac
done
mkdir -p "$out"
out=$(cd "$out" && pwd)
logs=${from:-$out}

rust_args=(cargo test --locked -p redlinedb-sql --test sqlite_full_parity --no-fail-fast)
version_args=(cargo test --locked -p redlinedb-sql --test sqlite_full_parity -- --exact reference_build_metadata_is_available --nocapture)
ffi_args=(cargo test --locked -p redlinedb-ffi --no-fail-fast)
probe_args=(bash scripts/compatibility/phase2-abi-probe.sh)
rust_command=${rust_args[*]}
ffi_command=${ffi_args[*]}
probe_command=${probe_args[*]}

# run <name> <command...>: log to $out/<name>.log, exit code to $out/<name>.exit.
run() {
  local name=$1 status=0
  shift
  (cd "$root" && "$@") >"$out/$name.log" 2>&1 || status=$?
  printf '%s\n' "$status" >"$out/$name.exit"
}

if [[ -z $from ]]; then
  export CARGO_INCREMENTAL=${CARGO_INCREMENTAL:-0}
  export CARGO_PROFILE_DEV_DEBUG=${CARGO_PROFILE_DEV_DEBUG:-line-tables-only}
  run rust-values "${rust_args[@]}"
  run sqlite-version "${version_args[@]}"
  # The ffi tests load the built library, and the probe links it.
  (cd "$root" && cargo build --locked -p redlinedb-ffi) >"$out/ffi-build.log" 2>&1 || true
  run ffi "${ffi_args[@]}"
  run abi-probe "${probe_args[@]}" --out "$out/abi-probe"
fi

# libtest's human output as TSV: binary, test, result (ok|failed|ignored).
tests_tsv() {
  awk '
    /^[[:space:]]*Running / { binary = ($2 == "unittests") ? $3 : $2; next }
    /^[[:space:]]*Doc-tests / { binary = "doc-tests " $2; next }
    /^test .* \.\.\. / {
      line = $0; sub(/^test /, "", line)
      at = index(line, " ... "); name = substr(line, 1, at - 1); result = substr(line, at + 5)
      split(result, word, /[ ,]/)
      if (word[1] == "ok") state = "ok"
      else if (word[1] == "FAILED") state = "failed"
      else if (word[1] == "ignored") state = "ignored"
      else next
      printf "%s\t%s\t%s\n", binary, name, state
    }' "$1"
}
# The "test result:" totals, as TSV: passed, failed, ignored.
summary_tsv() {
  awk '/^test result: / {
    for (i = 1; i <= NF; i++) {
      if ($(i + 1) ~ /^passed;?$/) p += $i
      if ($(i + 1) ~ /^failed;?$/) f += $i
      if ($(i + 1) ~ /^ignored;?$/) g += $i
    }
  } END { printf "%d\t%d\t%d\n", p, f, g }' "$1"
}
exit_of() { if [[ -f $1 ]]; then tr -d '[:space:]' <"$1"; else printf '%s' -1; fi; }
lock_version() { awk -v name="\"$1\"" '$1 == "name" && $3 == name { getline; gsub(/"/, "", $3); print $3; exit }' "$root/Cargo.lock"; }
commit=$(git -C "$root" rev-parse HEAD 2>/dev/null || printf unknown)
dirty=false
[[ -z $(git -C "$root" status --porcelain -- crates Cargo.lock Cargo.toml scripts contracts 2>/dev/null) ]] || dirty=true
generated_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)

# jq program shared by both lanes: a test lane from its TSV and summary.
lane_jq='
  def lane($command; $exit; $rows; $summary):
    ($rows | split("\n") | map(select(length > 0) | split("\t") | {binary: .[0], test: .[1], result: .[2]})) as $cases
    | ($summary | split("\t") | map(tonumber)) as $s
    | {
        command: $command,
        exit_code: $exit,
        passed: ($cases | map(select(.result == "ok")) | length),
        failed: ($cases | map(select(.result == "failed")) | length),
        ignored: ($cases | map(select(.result == "ignored")) | length),
        cases: $cases
      }
    | .status = (if .exit_code != 0 or .failed > 0 then "failed"
                 elif (.passed + .failed + .ignored) == 0 then "error"
                 elif [.passed, .failed, .ignored] != $s then "error"
                 else "passed" end);'

rust_rows=$(tests_tsv "$logs/rust-values.log")
rust_summary=$(summary_tsv "$logs/rust-values.log")
sqlite_version=$(sed -n 's/^sqlite_version=//p' "$logs/sqlite-version.log" | head -n 1)
jq -n "$lane_jq"'
  {
    schema: "redlinedb.qualification.sqlite-rust-values/v1",
    generated_at: $generated_at,
    source: {commit: $commit, dirty: ($dirty == "true")},
    surface: "Typed SQL values through the Rust API, compared in-process with the SQLite library that rusqlite bundles. Not the official SQL/CLI corpus, not the C ABI, not the file format.",
    oracle: {
      library: "SQLite bundled by rusqlite",
      sqlite_version: (if $sqlite_version == "" then null else $sqlite_version end),
      rusqlite: $rusqlite,
      libsqlite3_sys: $libsqlite3_sys
    },
    tests: lane($command; $exit; $rows; $summary)
  }
  | .status = (if .oracle.sqlite_version == null then "error" else .tests.status end)
' --arg generated_at "$generated_at" --arg commit "$commit" --arg dirty "$dirty" \
  --arg sqlite_version "$sqlite_version" --arg rusqlite "$(lock_version rusqlite)" \
  --arg libsqlite3_sys "$(lock_version libsqlite3-sys)" --arg command "$rust_command" \
  --argjson exit "$(exit_of "$logs/rust-values.exit")" --arg rows "$rust_rows" --arg summary "$rust_summary" \
  >"$out/sqlite_rust_values.json"

ffi_rows=$(tests_tsv "$logs/ffi.log")
ffi_summary=$(summary_tsv "$logs/ffi.log")
receipt=$logs/abi-probe/receipt.tsv
probe_rows=''
probe_header=''
if [[ -f $receipt ]]; then
  probe_rows=$(awk -F'\t' 'NR > 1 && $1 !~ /^#/ && $1 != "engine"' "$receipt")
  probe_header=$(sed -n 's/^# //p' "$receipt")
fi
jq -n "$lane_jq"'
  ($probe_rows | split("\n") | map(select(length > 0) | split("\t")
    | {engine: .[0], link: .[1], case: .[2], exit: (.[3] | tonumber), result: .[4]})) as $probe_cases
  | ($probe_header | split("\n") | map(select(length > 0) | split("\t") | {key: .[0], value: .[1:]}) | from_entries) as $identity
  | ($probe_cases | map(select(.engine == "sqlite"))) as $control
  | ($probe_cases | map(select(.engine == "redline"))) as $redline
  | {
      schema: "redlinedb.qualification.sqlite-c-abi/v1",
      generated_at: $generated_at,
      source: {commit: $commit, dirty: ($dirty == "true")},
      surface: "The sqlite3_* and rldb_* C ABI of libredlinedb: its Rust tests, and a C program compiled only against the vendored upstream SQLite 3.53.1 sqlite3.h. Not the official SQL/CLI corpus, not the file format.",
      ffi_tests: lane($ffi_command; $ffi_exit; $ffi_rows; $ffi_summary),
      abi_probe: {
        command: $probe_command,
        exit_code: $probe_exit,
        header_sha256: ($identity.header_sha256[0] // null),
        probe_source_sha256: ($identity.probe_source_sha256[0] // null),
        control: (if ($control | length) == 0 then {status: "skipped", reason: "no upstream libsqlite3 (scripts/sqlite/build-reference.sh builds one)"}
                  else {status: (if all($control[]; .result == "pass") then "passed" else "failed" end),
                        library_sha256: ($identity.control[1] // null),
                        passed: ($control | map(select(.result == "pass")) | length),
                        failed: ($control | map(select(.result != "pass")) | length)} end),
        redline: {
          library_sha256: ($identity.library[1] // null),
          passed: ($redline | map(select(.result == "pass")) | length),
          failed: ($redline | map(select(.result != "pass")) | length)
        },
        cases: $probe_cases
      }
    }
  | .abi_probe.status = (if .abi_probe.exit_code != 0 or .abi_probe.redline.failed > 0 then "failed"
                         elif .abi_probe.redline.passed == 0 then "error" else "passed" end)
  | .status = (if [.ffi_tests.status, .abi_probe.status] | all(. == "passed") then "passed"
               elif [.ffi_tests.status, .abi_probe.status] | any(. == "failed") then "failed" else "error" end)
' --arg generated_at "$generated_at" --arg commit "$commit" --arg dirty "$dirty" \
  --arg ffi_command "$ffi_command" --argjson ffi_exit "$(exit_of "$logs/ffi.exit")" \
  --arg ffi_rows "$ffi_rows" --arg ffi_summary "$ffi_summary" \
  --arg probe_command "$probe_command" --argjson probe_exit "$(exit_of "$logs/abi-probe.exit")" \
  --arg probe_rows "$probe_rows" --arg probe_header "$probe_header" \
  >"$out/sqlite_c_abi.json"

rust_status=$(jq -r .status "$out/sqlite_rust_values.json")
abi_status=$(jq -r .status "$out/sqlite_c_abi.json")
jq -r '"sqlite_rust_values: \(.status) (\(.tests.passed) passed, \(.tests.failed) failed, \(.tests.ignored) ignored; SQLite \(.oracle.sqlite_version // "unknown") via rusqlite \(.oracle.rusqlite))"' \
  "$out/sqlite_rust_values.json"
jq -r '"sqlite_c_abi: \(.status) (ffi tests \(.ffi_tests.passed) passed, \(.ffi_tests.failed) failed, \(.ffi_tests.ignored) ignored; probe \(.abi_probe.redline.passed) passed, \(.abi_probe.redline.failed) failed, control \(.abi_probe.control.status))"' \
  "$out/sqlite_c_abi.json"
[[ $rust_status == passed && $abi_status == passed ]]
