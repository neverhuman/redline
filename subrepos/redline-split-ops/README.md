# RedlineDB release tooling

[![CI](https://github.com/neverhuman/RedlineDB/actions/workflows/ci.yml/badge.svg)](https://github.com/neverhuman/RedlineDB/actions/workflows/ci.yml)

Agent entrypoint: [AGENTS.md](AGENTS.md).

This independent Rust workspace is included in
[neverhuman/RedlineDB](https://github.com/neverhuman/RedlineDB).
Source, reviews, CI and releases use that repository. Root `subrepos.toml`
defines all six components; no sibling clones or external control plane are needed.

## Quick start

From the complete checkout:

```sh
./subrepos/redline-split-ops/redlinectl validate
./subrepos/redline-split-ops/redlinectl validate --history
./subrepos/redline-split-ops/redlinectl family-ci
```

Validation checks component paths, dependency boundaries, GitHub authority and
retired forge routes. `--history` also verifies source trees, import commits and
preserved refs. `family-ci` runs root `scripts/ci-family.sh all` and propagates
failures. Running the installed controller outside the checkout requires
`REDLINE_REPO_ROOT` pointing to a complete source tree.

Use the [root release process](../../docs/release.md) for packages and GitHub
attestations. The old split lock, manifest and receipts are historical evidence
under `release-evidence/`; they never authorize a current release or deployment.
