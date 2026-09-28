#!/usr/bin/env bash
# Builds redlinedb-cli (the release profile) for a git ref, for a release
# bench bundle (L-05), and keeps only the binary and a record of the build:
#   target/version-history/<label>/redlinedb
#   target/version-history/<label>/build.json   (redline-version-build-v1)
# scripts/perf/release-bench.sh reads build.json beside a binary and
# declares that build in the bundle's build contract.
#
# Every version is built the same way. The build runs in a standalone
# `git clone --no-local` sandbox at <repo>/.agent/sandbox/<label>, never in
# /tmp and never as a git worktree, and the sandbox is removed on exit
# whether the build succeeds or fails. The ref's own rust-toolchain.toml,
# Cargo.lock and [profile.release] apply. RUSTFLAGS is set explicitly for
# every version (VERSION_BUILD_RUSTFLAGS, default empty), which makes cargo
# ignore the ref's .cargo/config.toml rustflags (older refs targeted
# x86-64-v3 there). CARGO_ENCODED_RUSTFLAGS, CARGO_PROFILE_* and
# CARGO_BUILD_* overrides are cleared. No profile is used, so there is no
# PGO. Default features only.
#
# Usage: scripts/perf/build-version.sh <git-ref> <label>
#   VERSION_BUILD_RUSTFLAGS  RUSTFLAGS for the build (default "", the portable baseline target);
#                            use the same value for every version in a bundle
#
# Refuses to overwrite an existing target/version-history/<label>/, and
# refuses a binary that carries failpoint names or debug-assertion markers.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(git -C "$script_dir" rev-parse --show-toplevel)"
source "$script_dir/lib.sh"

die() {
  printf 'build-version: %s\n' "$*" >&2
  exit 2
}

ref="${1:?usage: build-version.sh <git-ref> <label>}"
label="${2:?usage: build-version.sh <git-ref> <label>}"
[[ "$label" =~ ^[A-Za-z0-9][A-Za-z0-9._+-]*$ ]] || die "label must match ^[A-Za-z0-9][A-Za-z0-9._+-]*\$, got '$label'"
rustflags="${VERSION_BUILD_RUSTFLAGS-}"
case "$rustflags" in
  *profile-use*|*profile-generate*) die "VERSION_BUILD_RUSTFLAGS must not use a PGO profile: $rustflags" ;;
esac
command -v jq >/dev/null || die "jq is missing"

sha="$(git -C "$repo_root" rev-parse --verify --quiet "$ref^{commit}")" || die "$ref names no commit"
out_dir="$repo_root/target/version-history/$label"
[ ! -e "$out_dir" ] || die "$out_dir already exists; remove it to rebuild $label"
sandbox_root="$repo_root/.agent/sandbox"
sandbox="$sandbox_root/$label"
[ ! -e "$sandbox" ] || die "$sandbox exists: another build of $label is running, or one was killed; check and remove it"
git -C "$repo_root" check-ignore -q "$sandbox" \
  || die ".agent/ is not ignored here; list it in .git/info/exclude before building in $sandbox"

remove_sandbox() {
  local status=$?
  if [ -e "$sandbox" ]; then
    chmod -R u+rwX "$sandbox" 2>/dev/null || true
    rm -rf "$sandbox" || printf 'build-version: could not remove %s\n' "$sandbox" >&2
  fi
  rmdir "$sandbox_root" 2>/dev/null || true
  exit "$status"
}
mkdir -p "$sandbox_root"
trap remove_sandbox EXIT

printf '==> build-version: %s (%s) in %s\n' "$label" "$sha" "$sandbox"
git clone --quiet --no-local --no-checkout "$repo_root" "$sandbox"
git -C "$sandbox" cat-file -e "$sha^{commit}" 2>/dev/null \
  || die "$sha is not reachable from any branch or tag, so the clone lacks it"
git -C "$sandbox" checkout --quiet --detach "$sha"
[ "$(git -C "$sandbox" rev-parse HEAD)" = "$sha" ] || die "the sandbox is not at $sha"

cargo_command="cargo build --release --locked -p redlinedb-cli"
(
  cd "$sandbox"
  while IFS= read -r variable; do
    unset "$variable"
  done < <(env | sed -n 's/^\(CARGO_PROFILE_[A-Za-z0-9_]*\|CARGO_BUILD_[A-Za-z0-9_]*\)=.*/\1/p')
  unset CARGO_ENCODED_RUSTFLAGS CARGO_TARGET_DIR RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER
  export RUSTFLAGS="$rustflags" CARGO_INCREMENTAL=0
  $cargo_command
)
binary="$sandbox/target/release/redlinedb"
[ -x "$binary" ] || die "the build left no $binary"
perf_check_release_binary "$label" "$binary"

rustc_lines="$(cd "$sandbox" && rustc -vV | jq -R . | jq -s -c .)"
cargo_version="$(cd "$sandbox" && cargo -V)"
profile_release="$(awk '/^\[profile\.release\]/ { on = 1; print; next } /^\[/ { on = 0 } on' "$sandbox/Cargo.toml")"
cargo_config=null
if [ -f "$sandbox/.cargo/config.toml" ]; then
  cargo_config="$(jq -Rs . < "$sandbox/.cargo/config.toml")"
fi

mkdir -p "$out_dir"
install -m 0755 "$binary" "$out_dir/redlinedb"
jq -n --arg label "$label" --arg ref "$ref" --arg sha "$sha" \
  --arg commit_date "$(git -C "$sandbox" show -s --format=%cI "$sha")" \
  --argjson rustc "$rustc_lines" --arg cargo "$cargo_version" --arg command "$cargo_command" \
  --arg rustflags "$rustflags" --arg profile_release "$profile_release" \
  --argjson cargo_config "$cargo_config" \
  --arg binary_sha "$(sha256sum "$out_dir/redlinedb" | awk '{print $1}')" \
  --argjson size "$(stat -c %s "$out_dir/redlinedb")" \
  --arg built "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '{schema_version: "redline-version-build-v1", label: $label, source_ref: $ref,
    source_commit: $sha, source_commit_date: $commit_date,
    rustc_verbose_version: $rustc, cargo_version: $cargo, cargo_command: $command,
    profile: "release", features: "default", rustflags: $rustflags,
    cargo_encoded_rustflags_cleared: true, pgo: false,
    profile_release: $profile_release, cargo_config: $cargo_config,
    binary_sha256: $binary_sha, binary_size_bytes: $size, built_at_utc: $built,
    sandbox: "standalone git clone --no-local under .agent/sandbox, removed after the build"}' \
  > "$out_dir/build.json"
printf 'built %s -> %s (sha256 %s)\n' "$label" "$out_dir/redlinedb" "$(jq -r .binary_sha256 "$out_dir/build.json")"
