#!/usr/bin/env bash
# Independent C ABI probe. A C consumer compiled only against the vendored
# upstream SQLite 3.53.1 sqlite3.h (with -Wall -Wextra -Werror) calls the
# library under test; each case runs in its own process with a timeout.
# Upstream libsqlite3 runs the same upstream cases as a control of the probe.
#
# Usage: scripts/compatibility/phase2-abi-probe.sh [options]
#   --library PATH         shared library to test (default: build this checkout
#                          and use target/debug/libredlinedb.{so,dylib})
#   --static-archive PATH  also link the probe statically against this archive
#                          (default when building: target/debug/libredlinedb.a)
#   --control PATH|none    upstream libsqlite3 for the control run (default:
#                          target/sqlite-reference/3.53.1/lib/libsqlite3.so if
#                          present; otherwise the control is reported skipped)
#   --out DIR              receipts and logs (default: target/compatibility/abi)
# Exit status: 0 all cases passed, 1 a RedlineDB case failed, 2 infrastructure
# (bad header hash, compiler error, or a failing control).
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
header_dir=$root/contracts/c-abi/upstream/sqlite-3.53.1
source=$root/contracts/c-abi/probe/phase2_abi_probe.c
library='' archive='' control='' out=$root/target/compatibility/abi
while (($#)); do
  case $1 in
    --library) library=$2; shift 2 ;;
    --static-archive) archive=$2; shift 2 ;;
    --control) control=$2; shift 2 ;;
    --out) out=$2; shift 2 ;;
    *) sed -n '2,19p' "${BASH_SOURCE[0]}" >&2; exit 64 ;;
  esac
done

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }
infrastructure() { printf 'abi-probe infrastructure: %s\n' "$*" >&2; exit 2; }

want=$(cut -d' ' -f1 "$header_dir/SHA256SUMS")
[[ $(sha256 "$header_dir/sqlite3.h") == "$want" ]] || infrastructure "vendored sqlite3.h does not match SHA256SUMS"

case "$(uname -s)" in
  Darwin) dylib_ext=dylib; native_libs=(-liconv -lSystem -lresolv) ;;
  *) dylib_ext=so; native_libs=(-ldl -lpthread -lm) ;;
esac
if [[ -z $library ]]; then
  (cd "$root" && cargo build --locked -p redlinedb-ffi) || infrastructure "cargo build failed"
  library=$root/target/debug/libredlinedb.$dylib_ext
  [[ -n $archive ]] || archive=$root/target/debug/libredlinedb.a
fi
if [[ -z $control ]]; then
  control=$root/target/sqlite-reference/3.53.1/lib/libsqlite3.$dylib_ext
  [[ -f $control ]] || control=none
fi
[[ -f $library ]] || infrastructure "missing library $library"
[[ -z $archive || -f $archive ]] || infrastructure "missing archive $archive"

mkdir -p "$out"
out=$(cd "$out" && pwd)
cc=${CC:-cc}
flags=(-std=gnu11 -Wall -Wextra -Werror -O0 -g -I "$header_dir")
"$cc" "${flags[@]}" "$source" -ldl -o "$out/probe-dynamic" || infrastructure "probe does not compile against upstream sqlite3.h"
if [[ -n $archive ]]; then
  "$cc" "${flags[@]}" -DPROBE_STATIC "$source" "$archive" "${native_libs[@]}" -o "$out/probe-static" ||
    infrastructure "static probe does not link against $archive"
fi

# Cases every SQLite-compatible library must pass, then RedlineDB-only ones:
# upstream accepts the flags unsupported-flags expects refused, has no
# rldb_column_type, and faults on early-nul-guard's over-long bound.
upstream_cases=(v3-zero v3-persistent v3-normalize bounded-guard zero-guard negative-guard
  empty-tail embedded-nul error-output v3-error-output v2-tail repeat-tail type-tags
  text-conversions memory-open)
redline_cases=(unsupported-flags native-tags early-nul-guard)

receipt=$out/receipt.tsv
{
  printf '# header_sha256\t%s\n# probe_source_sha256\t%s\n' "$want" "$(sha256 "$source")"
  printf '# library\t%s\t%s\n' "$library" "$(sha256 "$library")"
  [[ -z $archive ]] || printf '# static_archive\t%s\t%s\n' "$archive" "$(sha256 "$archive")"
  [[ $control == none ]] || printf '# control\t%s\t%s\n' "$control" "$(sha256 "$control")"
  printf 'engine\tlink\tcase\texit\tresult\n'
} > "$receipt"

# Run one case in a fresh directory, killing it after 15 seconds.
run_case() {
  local engine=$1 link=$2 probe=$3 target=$4 case=$5 work status=0 ticks=0 timed_out=0 pid
  work=$(mktemp -d "$out/run.XXXXXX")
  (ulimit -c 0; exec "$probe" "$target" "$case" "$work/database" > "$out/$engine-$link-$case.stdout" 2> "$out/$engine-$link-$case.stderr") &
  pid=$!
  while kill -0 "$pid" 2>/dev/null; do
    if ((ticks >= 150 && !timed_out)); then kill -KILL "$pid" 2>/dev/null || true; timed_out=1; fi
    sleep 0.1
    ticks=$((ticks + 1))
  done
  wait "$pid" || status=$?
  rm -rf "$work"
  local result=failure
  if ((timed_out)); then result=timeout
  elif ((status == 0)); then result=pass
  elif ((status == 125)); then result=infrastructure
  elif ((status > 128)); then result=signal-$((status - 128))
  fi
  printf '%s\t%s\t%s\t%s\t%s\n' "$engine" "$link" "$case" "$status" "$result" >> "$receipt"
  printf '%-8s %-7s %-18s %s\n' "$engine" "$link" "$case" "$result"
  [[ $result == pass ]]
}

control_failed=0 redline_failed=0 control_runs=0
if [[ $control != none ]]; then
  for case in "${upstream_cases[@]}"; do
    control_runs=$((control_runs + 1))
    run_case sqlite dynamic "$out/probe-dynamic" "$control" "$case" || control_failed=$((control_failed + 1))
  done
fi
redline_runs=0
for case in "${upstream_cases[@]}" "${redline_cases[@]}"; do
  redline_runs=$((redline_runs + 1))
  run_case redline dynamic "$out/probe-dynamic" "$library" "$case" || redline_failed=$((redline_failed + 1))
  if [[ -n $archive ]]; then
    redline_runs=$((redline_runs + 1))
    run_case redline static "$out/probe-static" static "$case" || redline_failed=$((redline_failed + 1))
  fi
done

if [[ $control == none ]]; then
  printf 'control: skipped (no upstream libsqlite3; pass --control PATH)\n'
else
  printf 'control: %d/%d upstream cases passed\n' "$((control_runs - control_failed))" "$control_runs"
fi
printf 'redline: %d/%d case runs passed; receipt %s\n' "$((redline_runs - redline_failed))" "$redline_runs" "$receipt"
((control_failed == 0)) || infrastructure "the upstream control failed $control_failed case(s); the probe itself is suspect"
((redline_failed == 0)) || exit 1
