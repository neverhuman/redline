# GitHub authority

Follow [the root release process](../../../docs/release.md).
Root GitHub workflows generate packages, checksums and build attestations from
one tested parent commit. `target/` contains regenerated build and audit output.
Historical files in `release-evidence/` remain immutable data, not executable
configuration, publication prerequisites or current qualification evidence.
