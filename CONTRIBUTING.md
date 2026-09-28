# Contributing to RedlineDB

Thanks for the interest in RedlineDB.

## Prerequisites

Each lane needs the tools of the one above it. `scripts/ci-doctor.sh --profile
<name>` checks a profile and names what is missing; `just ci-doctor` runs the
required profile.

| Profile | Lanes | Tools |
|---|---|---|
| `core` | `./scripts/build-from-source.sh`, `cargo test -p <crate> --locked` | Rust 1.95.0 (rustup reads `rust-toolchain.toml`), a C compiler, pkg-config |
| `contributor` | `just fast`, `just clippy`, `just medium`, `./scripts/check_file_sizes.sh` | core, plus just, cargo-nextest 0.9.133, git, jq and curl. rtk is optional; the `just` lanes run commands directly without it. |
| `required` | `just required` (= `just pr-ci`), `just security`, `just redline-testing-official` | contributor, plus Node 22 and npm, Playwright's Chromium, Docker (or `REDLINE_TESTING_POSTGRES_URL`) for the PostgreSQL 16.15 lane, the pinned jankurai 1.6.11 (`bash ops/ci/install-github-tools.sh`), cargo-audit 0.22.1, cargo-deny 0.19.8 and gitleaks 8.21.2. Linux x86_64 only; CI runs it for every pull request. |

Running a release package needs none of these; see [docs/install.md](docs/install.md).
Jankurai and the audit lanes are described in
[docs/contributing/tooling.md](docs/contributing/tooling.md).

## Workflow

1. Keep changes scoped to the smallest lawful surface.
2. Add or update tests for behavior changes.
3. Run the active proof lane before asking for review.
4. Prefer small, reviewable commits with a clear intent.
5. Include raw evidence for failures: exit codes, failing test names, spans, advisory IDs, seeds, and log paths.

## Proof

- Default lane: `just fast`
- Wider lanes: `just clippy`, `just medium`, `just security`, `just release`
- Protected lane, as CI runs it: `just required`
- File-size gate: `./scripts/check_file_sizes.sh`
- Installer and packages: `bash scripts/test-installer.sh` needs no Rust;
  `bash scripts/test-native-install.sh <dir>` and
  `bash scripts/test-docs-quickstart.sh <dir>` run the installer and the
  documented quick start against archives from `scripts/package-release.sh`.

## Receipts

- Capture the exact command that failed.
- Keep the first failing output intact when reporting regressions.
- Note any skipped checks and why they were skipped.

## Notes

- The workspace is Apache-2.0 licensed.
- Avoid committing generated artifacts or local database state.
- If a change affects public APIs, update the README or other relevant docs.
- CI for a pull request from a fork runs on GitHub-hosted runners without the
  parity lane and the kernel test stage, so `RedlineDB/required` reports
  "Maintainer run required". After review, a maintainer runs the full CI on a
  copy of your commit (`docs/ci-trust-boundary.md`).
