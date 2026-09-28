#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
# shellcheck source=ops/ci/lib.sh
source ops/ci/lib.sh
# shellcheck source=ops/ci/security-lib.sh
source ops/ci/security-lib.sh
bash scripts/check-security-policy.sh
bash scripts/check-publish-policy.sh
CI_GITLEAKS_INSTALL_DIR="$root/target/ci/tools" ci_install_gitleaks
GITLEAKS="$root/target/ci/tools/gitleaks" bash ops/ci/tests/security-policy.sh "$root"
snapshot=$(mktemp -d)
trap 'rm -rf "$snapshot"' EXIT
git ls-files -z | tar --null -T - -cf - | tar -xf - -C "$snapshot"
"$root/target/ci/tools/gitleaks" detect --no-git --source "$snapshot" --config "$root/.gitleaks.toml" --redact
# Every tracked Cargo.lock outside test fixtures is a component. A component
# without a deny.toml beside it fails here unless ops/ci/security-exemptions.tsv
# exempts it; cargo-deny gets the component's own config explicitly, so it
# never falls back to a parent directory's deny.toml.
components=$(security_component_configs "$root")
while IFS=$'\t' read -r component config; do
  (cd "$component"; cargo audit --file Cargo.lock)
  if [[ $config == - ]]; then
    printf 'cargo deny skipped for %s: %s\n' "$component" "$(security_exemption_reason "$root" "$component")"
  else
    (cd "$component"; cargo deny check --config "$root/$config")
  fi
done <<<"$components"
npm --prefix subrepos/redline-web/apps/web ci
npm --prefix subrepos/redline-web/apps/web audit --audit-level=high
