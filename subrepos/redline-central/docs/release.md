# redline-central releases

Source, pull requests, CI and releases are owned by
[neverhuman/redline](https://github.com/neverhuman/redline).
This component is built from `subrepos/redline-central` in that checkout.
Its Cargo workspace stays independent; its release identity and artifacts come
from the reviewed parent commit and root release workflows.

Follow the [root release process](../../../docs/release.md).
Run `bash scripts/ci-family.sh all` from the root for acceptance. Packages and
attestations are published only by the root GitHub release workflow. Existing
releases are immutable, and installed consumer databases require explicit,
recoverable migrations. Historical receipts are evidence of past executions.
