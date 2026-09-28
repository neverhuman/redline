# Supply-chain and secret scans

How the repository checks its dependencies and history for known
vulnerabilities, untrusted sources and leaked secrets, and how a release
candidate gets a receipt that records the result.

## Components

Every tracked `Cargo.lock` is a component, except test fixtures (anything
under `ops/ci/tests/` or in a `fixtures/` directory). `ops/ci/security-lib.sh`
derives the list with `git ls-files '*Cargo.lock'`, so a new lockfile is
scanned without anyone editing a list.

Each component needs a `deny.toml` beside its `Cargo.lock`. The scans pass it
to cargo-deny with `--config`: cargo-deny would otherwise walk up the tree and
silently apply a parent directory's policy. Before v5 the component list was
typed into the script: redline-central's `cargo deny` was skipped because it
had no `deny.toml`, and the two nested tools under `subrepos/*/tools/` were
not scanned at all. A component
without its own `deny.toml` fails the scan unless
`ops/ci/security-exemptions.tsv` lists it with a reason; an exempt component
still gets `cargo audit`. The table is empty today.

Every component's `deny.toml` must deny yanked crates, unknown registries,
unknown git sources and wildcard version requirements (`yanked`,
`unknown-registry`, `unknown-git` and `wildcards` all `"deny"`). Versionless
path dependencies between crates that are `publish = false` stay allowed
through `allow-wildcard-paths = true`; `scripts/check-publish-policy.sh` keeps
the root workspace private. `multiple-versions` stays a warning.

## Secret scan

`.gitleaks.toml` extends gitleaks' default rules. Its allowlist names three
kinds of historical proof metadata that the generic API-key rule reports
(Cargo checksums, Git SHAs, request idempotency IDs). It uses
`regexTarget = "match"`: each regex is tested against the text the rule
matched, which starts at the key name, and not against the whole line. With
`"line"` a real secret on the same line as an allowlisted value was
suppressed.

## Proof that the policy refuses what it must

`bash ops/ci/tests/security-policy.sh` builds inputs from
`ops/ci/tests/fixtures/security/` and checks that each is refused, next to a
control that must pass:

| Case | Refused by |
| --- | --- |
| A `Cargo.lock` with no `deny.toml` and no exemption | `security_component_configs` |
| A `deny.toml` with `unknown-git = "warn"` | `security_component_configs` |
| A dependency from a git repository not in `allow-git` | `cargo deny check sources` |
| A publishable crate with a versionless path dependency | `cargo deny check bans` |
| A synthetic GitHub token on the same line as an allowlisted `key_sha256` | gitleaks with `.gitleaks.toml` |

The token is generated when the test runs, so no committed file holds one.

## CI

The `security` job runs `ops/ci/security-family.sh`: the security-policy and
publish-policy checks, the fixture tests above, a gitleaks scan of the
tracked files, `cargo audit` and `cargo deny check` for every component, and
`npm audit` for the web console.

## Release receipt

`bash ops/ci/security-receipt.sh` on the release candidate writes
`target/security/receipt.json` (schema `redline.security-receipt.v1`), with
the raw logs in `target/security/logs/` and the redacted gitleaks report in
`target/security/gitleaks-history.json`. The receipt records:

- the candidate SHA and ref, and whether the tree was dirty or the clone
  shallow;
- the versions of cargo-audit, cargo-deny and gitleaks;
- each advisory database the scans used: its HEAD, when that commit was made,
  when the database was last fetched, and its age in hours;
- the fixture test exit code;
- for every lockfile: its sha256, its `deny.toml` and that file's sha256 (or
  the exemption), the `cargo audit` exit code with vulnerability and warning
  counts, and the `cargo deny` exit code;
- `gitleaks git --log-opts=--all --redact` over the full history: exit code,
  finding count, commits scanned and the report's sha256;
- the reviewed exceptions: advisory ids ignored in any `deny.toml`, the
  exemption table, and the gitleaks allowlist with its target.

It exits non-zero, after writing the receipt, when a scan fails, when an
advisory database was last fetched more than 24 hours ago, when the clone is
shallow or when the tree has changes. `SECURITY_RECEIPT_ALLOW_DIRTY=1` and
`SECURITY_RECEIPT_ALLOW_SHALLOW=1` relax the last two for a local trial; the
receipt records them, and a release needs neither. Attaching the receipt to
the GitHub release and binding it into the release acceptance record is
separate work (CI-06).
