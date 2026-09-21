# Validation

Run `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`,
`cargo test --locked`, and `./redlinectl validate` in this component.
Run `./redlinectl validate --history` in a full Git checkout for custody checks.
Source archives support validation without Git history. The required family lane
is root `scripts/ci-family.sh all`, also used by root GitHub CI.

Failures return a nonzero status and identify the invalid manifest, component,
dependency or remote. Repair the named input and rerun the same command. Never
rewrite historical receipts or their hashes to make validation pass.

## Security and release evidence

`just security` runs dependency review (`cargo audit`, `cargo deny`), secret
scanning (`gitleaks`), workflow lint (`actionlint`, `zizmor`) and an SBOM (`syft`).
The release gate is root GitHub CI, which runs family security and publishes
immutable artifacts with build
attestations only after all required checks pass. Component validation alone is
not release readiness or platform qualification.

## Backups and recovery

Source backups use `git bundle create <custody-path> --all`. Restore independently
with `git clone --no-local --bare`, then verify exact commits, trees and
connectivity with `git cat-file`, `git rev-parse` and `git fsck`. Retain immutable
archives and receipts. `validate --history` checks original source trees and
preserved refs; it does not replace an independent recovery exercise.

## Monitoring and rollback

Monitor the root GitHub Actions run for the tested commit. Preserve command exit
codes and raw test/security logs; a failed, cancelled or skipped required job
blocks publication. Follow [ROLLBACK.md](../ROLLBACK.md) through a reviewed revert
and a new acceptance run. Never rewrite a published tag, artifact or old receipt.
This controller neither stores databases nor upgrades installed consumers.

## Abuse controls and repair

Reject checkout escapes, nested Git repositories, Git dependencies, alternate
fetch/push destinations and retired automation policies. Unknown publication
commands fail before invoking Git or a forge. The controller emits structured
repair fields (`purpose`, `reason`, `docs_url`, `repair_hint`, `common_fixes`) on
errors; correct the identified source/configuration and rerun the same proof lane.
