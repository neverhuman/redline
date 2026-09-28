#!/usr/bin/env bash
# Make the repository's pinned Rust toolchain usable in a CI job, going to
# the network only for what is missing (CI-04).
#
#   bash ops/ci/ensure-rust.sh
#
# The self-hosted runners already hold the toolchain, but
# dtolnay/rust-toolchain fetches the channel manifest from
# static.rust-lang.org in every job, and the runners' TLS link to it is
# flaky: in run 36298015185 four jobs failed in that step with 1.95.0
# already installed. This script checks offline first, with
# `rustup run <toolchain> rustc -V` and `rustup component list --installed`
# (RUSTUP_AUTO_INSTALL=0, so neither can start a download), and installs only
# a missing toolchain or component, retrying with a doubling delay.
#
# The toolchain and its components come from rust-toolchain.toml, which must
# pin an exact release. No global rustup setting changes (no `rustup default`,
# no self-update): the checkout's rust-toolchain.toml selects the toolchain.
# In a job it also exports CARGO_INCREMENTAL=0 (unless the job sets it) and
# CARGO_TERM_COLOR=always, as dtolnay/rust-toolchain does.
#
# Self-hosted jobs use this; GitHub-hosted jobs may use dtolnay/rust-toolchain.
# ops/ci/tests/ensure-rust.sh tests it against a rustup stand-in.
#
# Environment:
#   CI_RUST_TOOLCHAIN_FILE  toolchain file (default: the repository's)
#   CI_ENSURE_RUST_ATTEMPTS attempts per install (default 5)
#   CI_ENSURE_RUST_DELAY    seconds before the second attempt (default 15)
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
toolchain_file=${CI_RUST_TOOLCHAIN_FILE:-$root/rust-toolchain.toml}
attempts=${CI_ENSURE_RUST_ATTEMPTS:-5}
delay=${CI_ENSURE_RUST_DELAY:-15}

die() {
  printf 'ensure-rust.sh: %s\n' "$*" >&2
  exit 1
}

[[ -f $toolchain_file ]] || die "missing $toolchain_file"
# `key = value` from the [toolchain] table, quotes and brackets removed.
toml_value() {
  sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*//p" "$toolchain_file" | head -n 1 | tr -d "\"'[] "
}
toolchain=$(toml_value channel)
[[ $toolchain =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || die "$toolchain_file must pin an exact release (channel = \"X.Y.Z\"), not '${toolchain}'"
IFS=, read -r -a components <<< "$(toml_value components)"

if ! command -v rustup >/dev/null 2>&1; then
  for dir in "${CARGO_HOME:-}" "$HOME/.cargo"; do
    if [[ -n $dir && -x $dir/bin/rustup ]]; then
      export PATH="$dir/bin:$PATH"
      [[ -z ${GITHUB_PATH:-} ]] || printf '%s\n' "$dir/bin" >> "$GITHUB_PATH"
      break
    fi
  done
fi
command -v rustup >/dev/null 2>&1 \
  || die "rustup is not installed on this runner (checked PATH, \$CARGO_HOME/bin and ~/.cargo/bin); install rustup on the host"

has_toolchain() {
  local reported
  reported=$(RUSTUP_AUTO_INSTALL=0 rustup run "$toolchain" rustc -V 2>/dev/null) || return 1
  [[ $reported == "rustc $toolchain "* ]]
}

# The components rust-toolchain.toml names that the toolchain lacks.
missing_components() {
  local installed component
  installed=$(RUSTUP_AUTO_INSTALL=0 rustup component list --installed --toolchain "$toolchain" 2>/dev/null) || return 1
  for component in ${components[@]+"${components[@]}"}; do
    grep -Eq "^${component}(-|$)" <<< "$installed" || printf '%s\n' "$component"
  done
}

retry() {
  local attempt=1 wait=$delay
  until RUSTUP_MAX_RETRIES=${RUSTUP_MAX_RETRIES:-10} "$@"; do
    if ((attempt >= attempts)); then
      printf 'ensure-rust.sh: "%s" failed after %d attempts\n' "$*" "$attempts" >&2
      return 1
    fi
    printf '::warning::"%s" failed (attempt %d of %d); retrying in %ss\n' "$*" "$attempt" "$attempts" "$wait" >&2
    sleep "$wait"
    attempt=$((attempt + 1))
    wait=$((wait * 2))
  done
}

if has_toolchain; then
  printf 'Rust %s is installed; no download needed.\n' "$toolchain"
else
  printf 'Rust %s is not installed; installing it.\n' "$toolchain"
  install=(rustup toolchain install "$toolchain" --profile minimal --no-self-update)
  if ((${#components[@]})); then
    install+=(--component "$(IFS=,; printf '%s' "${components[*]}")")
  fi
  retry "${install[@]}"
fi

missing=$(missing_components) || die "cannot list the components of Rust $toolchain"
if [[ -n $missing ]]; then
  # shellcheck disable=SC2086 # one component per word
  retry rustup component add --toolchain "$toolchain" $missing
fi

has_toolchain || die "Rust $toolchain does not run or reports another version: $(RUSTUP_AUTO_INSTALL=0 rustup run "$toolchain" rustc -V 2>&1 | head -n 1)"
missing=$(missing_components) || die "cannot list the components of Rust $toolchain"
[[ -z $missing ]] || die "Rust $toolchain still lacks: $(tr '\n' ' ' <<< "$missing")"

if [[ -n ${GITHUB_ENV:-} ]]; then
  [[ -n ${CARGO_INCREMENTAL:-} ]] || printf 'CARGO_INCREMENTAL=0\n' >> "$GITHUB_ENV"
  printf 'CARGO_TERM_COLOR=always\n' >> "$GITHUB_ENV"
fi
printf 'Using %s with components: %s\n' "$(RUSTUP_AUTO_INSTALL=0 rustup run "$toolchain" rustc -V)" "${components[*]:-none}"
