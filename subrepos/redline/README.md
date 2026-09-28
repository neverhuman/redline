> Source, reviews, CI and releases: [neverhuman/redline](https://github.com/neverhuman/redline).
> This component lives at `subrepos/redline` in the canonical checkout.
> Follow the [root release process](../../docs/release.md); historical component release examples below are archival.

# RedlineDB

`redline` is the historical public hub for the Redline family. The family is
now a single GitHub repository: the engine lives in
[`crates/`](../../crates) at the root of the canonical checkout, the
conformance harness in [`redline-testing`](../redline-testing), and the
observability console in [`redline-web`](../redline-web).

This directory intentionally contains no Cargo workspace and no engine source.
Release tooling lives in [`redline-split-ops`](../redline-split-ops). The
separate family manifest and lock file were retired with the multi-repo
layout; [neverhuman/redline](https://github.com/neverhuman/redline) is now
the sole source and release authority.

## Public entry points

- Engine API and CLI: [`crates/`](../../crates)
- SQLite-parity, RQL, memory, and beyond-SQLite evidence:
  [`redline-testing`](../redline-testing)
- SQL console and metrics dashboard: [`redline-web`](../redline-web)
- Release tooling: [`redline-split-ops`](../redline-split-ops)

Run `scripts/guard-no-duplicate-engine.sh` from this repository before
publishing a hub change. It fails if engine crates or a Cargo workspace are
reintroduced here.
