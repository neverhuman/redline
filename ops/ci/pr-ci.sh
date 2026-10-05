#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
bash scripts/ci-family.sh all
bash ops/ci/install-github-tools.sh
target/ci/tools/jankurai security run . --strict --profile ci --script ops/ci/security-family.sh --out target/jankurai/security/evidence.json
bash ops/ci/audit-family.sh
# Mirror packages-cross.yml without mixing ARM64 and native archives.
OUTPUT_DIR="$root/target/ci/arm64-packages" timeout 5400 bash ops/ci/arm64-packages.sh build
ARM64_PACKAGES_DIR="$root/target/ci/arm64-packages" timeout 2700 bash ops/ci/arm64-packages.sh runtime
