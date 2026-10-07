#!/usr/bin/env bash
# Bounded checks for the independent workspace; this is not a fuzz campaign.
set -euo pipefail
cargo test --manifest-path fuzz/Cargo.toml --lib
cargo check --manifest-path fuzz/Cargo.toml --bins
node docs/audits/v5.1.1-post-release-evidence/verify.mjs
