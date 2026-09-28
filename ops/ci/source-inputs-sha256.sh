#!/usr/bin/env bash
# Print the SHA-256 of the source inputs of the checkout in the current
# directory: the Git tree entries at HEAD of every path that can change what
# the engine, the corpus, the oracle or the CI policy measure. Generated
# reports, charts and README edits are not inputs, so publishing a report
# does not change the hash.
#
#   (cd <checkout> && bash ops/ci/source-inputs-sha256.sh)
#
# ops/ci/sqlite-parity-report.sh commits it as
# .github/parity-report-inputs.sha256; redline-testing records the same hash
# as source_inputs_sha256 in its run provenance (SOURCE_INPUT_PATHS in
# subrepos/redline-testing/src/evidence/identity.rs must list these paths, and
# a unit test there reads this recipe); ops/ci/release-acceptance.sh binds it
# into release-acceptance.v1.json.
set -euo pipefail
git ls-tree -r HEAD -- Cargo.toml Cargo.lock rust-toolchain.toml .cargo crates subrepos metadata ops scripts \
  agent/audit-policy.toml .jankurai/audit-policy.toml .github/workflows/ci.yml \
  .github/workflows/sqlite-parity-report.yml | sha256sum | cut -d ' ' -f 1
