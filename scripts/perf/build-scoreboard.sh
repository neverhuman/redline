#!/usr/bin/env bash
# Builds the engine-throughput scoreboard (crates/scoreboard) against the
# engine of a git ref, and keeps the binary and a record of the build:
#   target/scoreboard/<label>/redline-scoreboard
#   target/scoreboard/<label>/build.json   (redline-scoreboard-build-v1)
# scripts/perf/scoreboard-bench.sh measures those binaries into a bundle.
#
# The scoreboard links the engine as a library, so measuring a version
# means building the harness against that version's source. The harness
# itself comes from one commit (--harness-ref, default HEAD) for every
# version: its crates/scoreboard is copied over the version's tree and
# added to the workspace. Only the engine differs between versions in a
# bundle, and build.json records the harness tree so the summarizer can
# refuse a bundle whose harnesses differ.
#
# As in build-version.sh, the build runs in a standalone
# `git clone --no-local` sandbox under .agent/sandbox, never in /tmp and
# never as a git worktree, and the sandbox is removed on exit. Only
# `-p redlinedb-scoreboard` is built, so no other workspace member can turn
# the kernel's failpoints on, and the binary is refused if it carries
# failpoint names or debug-assertion markers. RUSTFLAGS is explicit
# (VERSION_BUILD_RUSTFLAGS, default empty); no PGO.
#
# Usage: scripts/perf/build-scoreboard.sh <engine-ref> <label> [--harness-ref <ref>]

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(git -C "$script_dir" rev-parse --show-toplevel)"
source "$script_dir/lib.sh"

die() {
  printf 'build-scoreboard: %s\n' "$*" >&2
  exit 2
}

usage='usage: build-scoreboard.sh <engine-ref> <label> [--harness-ref <ref>]'
ref="${1:?$usage}"
label="${2:?$usage}"
shift 2
harness_ref=HEAD
while [ $# -gt 0 ]; do
  case "$1" in
    --harness-ref) harness_ref="${2:?$usage}"; shift 2 ;;
    *) die "$usage" ;;
  esac
done
[[ "$label" =~ ^[A-Za-z0-9][A-Za-z0-9._+-]*$ ]] || die "label must match ^[A-Za-z0-9][A-Za-z0-9._+-]*\$, got '$label'"
rustflags="${VERSION_BUILD_RUSTFLAGS-}"
case "$rustflags" in
  *profile-use*|*profile-generate*) die "VERSION_BUILD_RUSTFLAGS must not use a PGO profile: $rustflags" ;;
esac
command -v jq >/dev/null || die "jq is missing"

sha="$(git -C "$repo_root" rev-parse --verify --quiet "$ref^{commit}")" || die "$ref names no commit"
harness_sha="$(git -C "$repo_root" rev-parse --verify --quiet "$harness_ref^{commit}")" \
  || die "$harness_ref names no commit"
harness_tree="$(git -C "$repo_root" rev-parse --verify --quiet "$harness_sha:crates/scoreboard")" \
  || die "$harness_ref has no crates/scoreboard"
out_dir="$repo_root/target/scoreboard/$label"
[ ! -e "$out_dir" ] || die "$out_dir already exists; remove it to rebuild $label"
sandbox_root="$repo_root/.agent/sandbox"
sandbox="$sandbox_root/scoreboard-$label"
[ ! -e "$sandbox" ] || die "$sandbox exists: another build of $label is running, or one was killed; check and remove it"
git -C "$repo_root" check-ignore -q "$sandbox" \
  || die ".agent/ is not ignored here; list it in .git/info/exclude before building in $sandbox"

remove_sandbox() {
  local status=$?
  if [ -e "$sandbox" ]; then
    chmod -R u+rwX "$sandbox" 2>/dev/null || true
    rm -rf "$sandbox" || printf 'build-scoreboard: could not remove %s\n' "$sandbox" >&2
  fi
  rmdir "$sandbox_root" 2>/dev/null || true
  exit "$status"
}
mkdir -p "$sandbox_root"
trap remove_sandbox EXIT

printf '==> build-scoreboard: %s (engine %s, harness %s) in %s\n' "$label" "$sha" "$harness_sha" "$sandbox"
git clone --quiet --no-local --no-checkout "$repo_root" "$sandbox"
git -C "$sandbox" cat-file -e "$sha^{commit}" 2>/dev/null \
  || die "$sha is not reachable from any branch or tag, so the clone lacks it"
git -C "$sandbox" checkout --quiet --detach "$sha"
[ "$(git -C "$sandbox" rev-parse HEAD)" = "$sha" ] || die "the sandbox is not at $sha"

# The harness, from one commit for every version.
rm -rf "$sandbox/crates/scoreboard"
git -C "$repo_root" archive "$harness_sha" crates/scoreboard | tar -x -C "$sandbox"
if ! grep -q '"crates/scoreboard"' "$sandbox/Cargo.toml"; then
  sed -i 's|^members = \[$|members = [\n    "crates/scoreboard",|' "$sandbox/Cargo.toml"
  grep -q '"crates/scoreboard"' "$sandbox/Cargo.toml" || die "could not add crates/scoreboard to the workspace"
fi

cargo_command="cargo build --release --offline -p redlinedb-scoreboard"
(
  cd "$sandbox"
  while IFS= read -r variable; do
    unset "$variable"
  done < <(env | sed -n 's/^\(CARGO_PROFILE_[A-Za-z0-9_]*\|CARGO_BUILD_[A-Za-z0-9_]*\)=.*/\1/p')
  unset CARGO_ENCODED_RUSTFLAGS CARGO_TARGET_DIR RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER
  export RUSTFLAGS="$rustflags" CARGO_INCREMENTAL=0
  $cargo_command
)

# The lock may only gain the harness package: a new registry package
# would carry a source and checksum line, and nothing may be removed.
lock_delta="$(git -C "$sandbox" diff -U0 -- Cargo.lock | grep -E '^[-+]' | grep -vE '^(\+\+\+|---) ' || true)"
if printf '%s\n' "$lock_delta" | grep -qE '^-|^\+(source|checksum) '; then
  printf '%s\n' "$lock_delta" >&2
  die "building the harness changed Cargo.lock beyond adding redlinedb-scoreboard"
fi

binary="$sandbox/target/release/redline-scoreboard"
[ -x "$binary" ] || die "the build left no $binary"
perf_check_release_binary "$label" "$binary"
info="$("$binary" info)"

rustc_lines="$(cd "$sandbox" && rustc -vV | jq -R . | jq -s -c .)"
cargo_version="$(cd "$sandbox" && cargo -V)"
profile_release="$(awk '/^\[profile\.release\]/ { on = 1; print; next } /^\[/ { on = 0 } on' "$sandbox/Cargo.toml")"

mkdir -p "$out_dir"
install -m 0755 "$binary" "$out_dir/redline-scoreboard"
jq -n --arg label "$label" --arg ref "$ref" --arg sha "$sha" \
  --arg commit_date "$(git -C "$sandbox" show -s --format=%cI "$sha")" \
  --arg harness_ref "$harness_ref" --arg harness_sha "$harness_sha" --arg harness_tree "$harness_tree" \
  --argjson info "$info" --arg lock_delta "$lock_delta" \
  --argjson rustc "$rustc_lines" --arg cargo "$cargo_version" --arg command "$cargo_command" \
  --arg rustflags "$rustflags" --arg profile_release "$profile_release" \
  --arg binary_sha "$(sha256sum "$out_dir/redline-scoreboard" | awk '{print $1}')" \
  --arg built "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '{schema_version: "redline-scoreboard-build-v1", label: $label,
    engine_ref: $ref, engine_commit: $sha, engine_commit_date: $commit_date,
    harness_ref: $harness_ref, harness_commit: $harness_sha, harness_tree: $harness_tree,
    engines: $info, cargo_lock_delta: $lock_delta,
    rustc_verbose_version: $rustc, cargo_version: $cargo, cargo_command: $command,
    profile: "release", rustflags: $rustflags, pgo: false, failpoints: false,
    profile_release: $profile_release,
    binary_sha256: $binary_sha, built_at_utc: $built,
    sandbox: "standalone git clone --no-local under .agent/sandbox, removed after the build"}' \
  > "$out_dir/build.json"
printf 'built %s -> %s (sha256 %s)\n' "$label" "$out_dir/redline-scoreboard" "$(jq -r .binary_sha256 "$out_dir/build.json")"
