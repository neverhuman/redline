# Contributor tooling: jankurai

The repository audit uses jankurai, pinned in `ops/ci/lib.sh` to version
1.6.11 and a SHA-256 of the binary. `bash ops/ci/install-github-tools.sh`
downloads that build (Linux x86_64 only), checks both digests and installs it
as `target/ci/tools/jankurai`; the CI lanes call that path, not whatever
`jankurai` is on `PATH`. `scripts/ci-doctor.sh --profile required` reports
whether it is there with the pinned digest.

This repository is already set up for jankurai; do not run its scaffolding
commands here. The lanes that use it:

| Command | What it runs |
|---|---|
| `just score` (report maintenance, not a verification command) | `jankurai audit` with `agent/audit-policy.toml`, writing `.jankurai/repo-score.{json,md}` and the score history |
| `just doctor` | `jankurai doctor --fail-on high` |
| `just rust-map`, `just rust-witness`, `just rust-diagnose` | `jankurai rust map .`, `jankurai rust witness build .`, `jankurai rust diagnose .` |
| `just required` | the protected lane, whose audit step runs the pinned binary |

For an ad hoc audit, write the reports to a scratch path and pass
`--no-score-history`, so the committed reports and history stay untouched
(`AGENTS.md` explains why). The canonical standard is
`.jankurai/JANKURAI_STANDARD.md`.

For Rust services that want runtime repair packets, the optional `witness-rt`
crate can emit packets that feed the Rust witness and diagnose flows.
