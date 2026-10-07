#!/usr/bin/env bash
# No timings in CI: verify committed full-work receipts and their derivations.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
bundle=benchmark-results/perf/regressions/main-bac7e9186-vs-v5.1.1-abba
node --test scripts/perf/abba-summary.test.mjs
node scripts/perf/abba-summary.mjs "$bundle" --check
node scripts/perf/verify-readme-figures.mjs
bash ops/ci/tests/arm64-packages.sh
if rg -q '<!-- main-regression:begin -->' README.md; then
  node scripts/perf/abba-summary.mjs "$bundle" --check README.md
fi
node --test scripts/perf/focused-summary.test.mjs
node --test scripts/perf/focused-investigation.test.mjs
node scripts/perf/focused-investigation.mjs benchmark-results/perf/regressions/focused-normal-94da-20261007 --check
