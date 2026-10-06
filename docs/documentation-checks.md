# Checking documentation against a release

Run `bash scripts/check-docs.sh` from the repository root. CI runs the same
command in the fast preflight, so it is included in `just fast` and
`just pr-ci`. It requires Node 22, curl, tar and the repository's Rust
toolchain. Published packages are cached in ignored `target/docs-reference/`;
Fixture databases and compiled examples use a private lane `target/` directory
or the caller’s non-`/tmp` `TMPDIR`. Helpers retain the caller’s `HOME`;
manual example files are copied into the fixture rather than linked to source. The preflight
uploads the raw documentation and ABI receipts as `documentation-checks`.

The release contract in `scripts/docs/release-contract.json` pins the
published v5.1.1 and v5.1.0 core archive digests for all four native platforms.
Every run verifies the archive before extracting it, checks the installed
build-info against the tag and commit, and compares CLI/server help with the
captured release output. It uses the published executable, including its
compatibility limitations. A freshly built development CLI is not the oracle.

## Executable examples

A `sql doctest` or `bash doctest` fence runs in a fresh private directory.
`-- prints: ...` in SQL and `# prints: ...` in Bash give the exact stdout,
including row order and line endings. `postgres` on the fence sets the result
dialect, and `crlf` requires CRLF output. `error` requires exit 1 and an
`-- error: ...` diagnostic. Unexpected errors, extra output and wrong results
fail the check. The new release/upgrade/limitations guides also bind tag commits, workflow run ids, the source-input hash and dated issue count to the committed receipts. The regression tests deliberately change results and introduce
invalid SQL and unknown flags to verify that these failures are detected.

Rust fences marked `rust doctest` compile and run against the workspace
facade. Their `// prints: ...` markers describe stdout. Short fragments in
the parameters and transaction chapters get a hidden setup that creates the
`note` table, connection and `body`, then queries the result. The embedding
example gets a `main` that calls its `open` function. These test the current
Rust API separately from the published CLI and C library. The README Rust
block is already compared byte for byte with its Cargo example and run twice
by preflight.

The checker also executes the RQL document, `examples/first.sql`, a
v5.1.0-to-v5.1.1 physical-backup round trip that runs the actual `bash upgrade` fence after substituting private fixture paths, durability/dialect configuration
probes and a private TCP server protocol round trip. It executes the pinned installer quick starts without a development toolchain
and tests the published C library through the existing package ABI probe.
The package lane separately checks freshly built candidate archives.

## Templates, history and scope

`bash template` fences are operator recipes. They may build, publish, remove
an installation, use an application-owned path or require credentials. The
checker parses their shell syntax after replacing explicit angle-bracket
placeholders; it does not execute a publication or touch a live database.
Existing unannotated templates are recorded by content digest and reason in
`scripts/docs/scope.json`. Changing one requires an explicit classification
or conversion into an executable example; it cannot silently become a pass.

The scope registry covers README, CHANGELOG and every Markdown page in
`docs/`. Diagrams, layouts and transcripts have explicit non-execution
reasons. Archived design, dated releases, migration records and unverified
historical measurements remain historical, with their original evidence and
limitations. Grok/Claude own their active audit and performance work; this
check does not present those plans as a measured release result.

The receipt at `target/docs-check/receipt.json` reports executions, parsed
templates, unexecuted blocks, delegated checks and failures separately. The README Rust block is a byte comparison in this checker; its compilation and two executions belong to the subsequent fast preflight, not this receipt. A parsed template or
historical exclusion is not a runtime pass. Read that scope before making a
claim that every feature or deployment scenario has been tested.

## Generated compatibility and performance claims

The limitations page is generated from the authored template
`scripts/docs/known-limitations.md.in`, the committed processed official
evidence and parity policies. Edit the template, not the generated page.
The README and RQL guide's coverage paragraphs use those same inputs. Regenerate it with
`node scripts/docs/render-coverage.mjs`, then run the check; missing blocks
or changed counts fail. RQL totals and skip IDs are checked against the same
evidence. An expected rejection or declared-unsupported case is not a
successful query, and a skip is not a pass.

The README and release-note benchmark blocks retain their existing
publishable-bundle renderers and CI drift tests. Never edit their numbers
by hand or replace them with a diagnostic run. The current engine table and
the startup-inclusive historical CLI table measure different workloads.
