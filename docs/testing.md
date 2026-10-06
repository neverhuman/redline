# Testing — Proof Lanes, Budgets, and Repair Receipts

Every change in this repo is validated through a named *proof lane* —
a deterministic recipe an agent can rerun without re-discovering it.
Lanes are declared in `.jankurai/proof-lanes.toml`; this doc indexes them,
distinguishes budget policy from implemented stop controls, and indexes
the optional structured error helpers and recorded proof receipts.

<a id="proof-lanes"></a>

## Proof-lane index

| Lane                                 | Proves                                                                                                |
|--------------------------------------|--------------------------------------------------------------------------------------------------------|
| `setup`                              | Prime the workspace build cache before a wider proof run.                                             |
| `check`                              | Root validation gate: fast, score, security, rust-map, rust-witness, and rust-diagnose.              |
| `test`                               | Fast workspace test proof.                                                                            |
| `verify`                             | Alias for the root validation gate.                                                                   |
| `fast`                               | Workspace fmt, file-size policy, type-check, and full unit/integration test sweep. Uses `scripts/sccache_wrapper.sh`, which falls back cleanly when local `sccache` is absent. Quick iteration lane, not the pre-push gate. |
| `pr-ci`                              | Complete local engine/component/conformance/security/audit/package gates through `scripts/ci-local.sh pr-ci`; GitHub adds Linux ARM64 cross-build/emulation; macOS packaging is required for tags. Local PostgreSQL needs `REDLINE_TESTING_POSTGRES_URL`. |
| `fast-check`                         | Workspace compile proof for the default health lane.                                                  |
| `fast-test`                          | Workspace test proof for the default health lane.                                                     |
| `hygiene`                            | Format and file-size only; cheapest pre-commit gate.                                                  |
| `clippy`                             | `cargo clippy --workspace --all-targets -- -D warnings`.                                              |
| `medium`                             | `fast` plus `--help` smoke for `cli` and `server`.                                                    |
| `phase8-smoke`                       | Same as `medium`; pinned for phase-8 regression triage.                                               |
| `kernel-cursor`                      | Cursor-specific kernel regression tests without the full workspace sweep.                             |
| `cache-warm`                         | Prime the workspace build cache before a wider proof run.                                             |
| `redline-testing-official`           | Official conformance and benchmark gate built from `subrepos/redline-testing` at the same parent commit as the engine. |
| `official-evidence-guard`            | Fails if RedlineDB reintroduces official metric/report generation outside the included, hash-verified runner or its processed evidence bundle. |
| `sqlite-parity-report-update`        | Regenerates official SQLite parity report and README data through the included `redline-testing` runner and processed evidence bundle. |
| `ffi-abi`                            | C ABI compatibility tests for the SQLite shim surface.                                                |
| `cli-shell`                          | CLI compatibility tests for the shell/batch front end.                                                |
| `kernel-check`                       | Targeted `redlinedb-kernel` compile proof.                                                            |
| `kernel-test`                        | Targeted `redlinedb-kernel` test proof.                                                                |
| `sql-check`                          | Targeted `redlinedb-sql` compile proof.                                                               |
| `sql-test`                           | Targeted `redlinedb-sql` test proof.                                                                   |
| `beyond-sqlite-manifest`             | Verifies the beyond-SQLite backlog ranking, source tips, owners, and proof-lane routing.               |
| `beyond-postgres-reference`          | Runs the beyond-SQLite manifest and Postgres oracle tests against pinned PostgreSQL 16.15. Starts a Docker container locally when `REDLINEDB_POSTGRES_URL` is unset. |
| `ffi-check`                          | Targeted `redlinedb-ffi` compile proof.                                                               |
| `ffi-test`                           | Targeted `redlinedb-ffi` test proof.                                                                   |
| `cli-check`                          | Targeted `redlinedb-cli` compile proof.                                                               |
| `cli-test`                           | Targeted `redlinedb-cli` test proof.                                                                   |
| `phase9-smoke`                       | Bench harness unit tests plus a one-rep certify and a cross-engine sweep.                                   |
| `phase9-compat-full`                 | Full `cross-engine --engine both` matrix against `crates/bench/compat/`.                                    |
| `phase9-certification`               | 5-rep + 1-warmup certify against `crates/bench/bench/certification.toml`.                             |
| `phase9-xbabe1-gap`                  | Gap certify on the xbabe1 docker host (sync, run, fetch).                                             |
| `phase9-xbabe1-gap-strace`           | Same as above with `strace -c` aggregation.                                                           |
| `phase9-docker-smoke`                | Bench unit tests run inside the xbabe1 docker host.                                                   |
| `phase9-recovery-matrix`             | Recovery matrix run for WAL/checksum failure modes.                                                   |
| `phase9-failpoint-matrix`            | Failpoint matrix run.                                                                                 |
| `phase9-xbabe1-certification`        | Full 5-rep certify on the xbabe1 host.                                                                |
| `phase9-xbabe1-certify-with-strace`  | Same as above with strace-instrumented children.                                                      |
| `phase10-xbabe1-certification`       | Phase-10 closing certification matrix on the 128-core xbabe1 host.                                    |
| `phase10-hnsw-recall`                | HNSW recall test (`-- --ignored`).                                                                    |
| `phase11-oltp-gap`                   | OLTP gap workload certify.                                                                            |
| `phase11-ephemeral-db`               | Ephemeral DB integration test.                                                                        |
| `phase11-sql-contracts`              | Phase-11 SQL contract tests (temp roots, queue, xdoug-compat).                                        |
| `security`                           | `cargo audit` + `cargo deny check` + `gitleaks detect`.                                               |
| `security-local`                     | Same as `security`; pinned for local-only invocation.                                                 |
| `release-binary-smoke`               | Builds and verifies the pinned RedlineDB `v5.0.0` Linux release package, then runs a CLI smoke query. |
| `release`                            | `cargo build --workspace --release --locked`.                                                         |
| `jankurai-tools`                     | Local mirror for every `.github/workflows/jankurai-tools.yml` matrix job. Run with `scripts/ci-local.sh jankurai-tools`. |
| `pr-gate`                            | Local mirror for PR branch freshness plus `jankurai staged-gate` against `origin/main`. Run with `scripts/ci-local.sh pr-gate`. |

Lane definitions: `.jankurai/proof-lanes.toml`. To rerun a lane:

```
rtk just <lane-name>
```

(or invoke the command list from the TOML directly).

SQLite parity boundary: the official evidence flow lives in
[`docs/sqlite-parity.md`](sqlite-parity.md), and `redline-testing-official`
is the only lane that produces committed parity evidence. RedlineDB does not
expose a local SQLite parity coverage/benchmark/report/sentinel producer; the
in-tree `sqlite_parity` commands and prior parity bundle workflows fail closed.
The retired quick/medium replay, case-list, and local diff helpers are not
release lanes; performance measurements use `perf-full` or the official report
workflow through the included, hash-verified runner.
The proof-lane definitions and audit policy remain pinned in
`.jankurai/proof-lanes.toml` and `agent/audit-policy.toml`.

The official lane builds `subrepos/redline-testing` with its preserved lockfile,
stages its corpus and metadata, and binds evidence to the runner binary digest.
No separately released runner or sibling checkout is required. Run it with:

```bash
just redline-testing-official
```

Set `REDLINEDB_SQLITE_PARITY_SQLITE_BIN=/path/to/sqlite3` to run
`redline-testing-official` against a pinned SQLite shell with optional shell and
extension features enabled. If unset, the official wrapper builds the SQLite
`3.53.1` autoconf shell through `scripts/sqlite/build-reference.sh` and exports
`target/sqlite-reference/3.53.1/bin/sqlite3` before comparing cases. The
builder verifies the upstream SHA3-256 digest and smokes percentile, math,
FTS5, RTREE, DBSTAT, `generate_series`, and `uint` support.

For narrow repair loops, prefer the package-scoped lanes above over `fast` when the touched surface is already known. They stay deterministic without forcing a workspace-wide run.

Certification recipes must build the benchmark with `--release`: measured
child processes run the same executable, so a debug build cannot produce a
publishable certification. This is the optimized smoke recipe used by the
`phase9-smoke` lane:

```bash template
cargo run -p redlinedb-bench --release -- certify --config crates/bench/bench/smoke.toml --out-dir target/bench/certify-smoke --seed 7 --repetitions 1 --warmup 0
```

It is an operator template, not a measurement performed by the documentation
check. A smoke run does not replace the quiet-host scoreboard bundle required
for README performance claims.

The optional `beyond-postgres-reference` lane starts its own local oracle:

```
rtk just beyond-postgres-reference
```

If `REDLINEDB_POSTGRES_URL` is set, the lane uses that database. Otherwise it
starts the digest-pinned PostgreSQL 16.15 bookworm image in
`ops/ci/beyond-postgres-reference.sh` (an override with a different digest is refused) with database
`redlinedb_beyond`, user `redlinedb`, password `postgres`, and local port
`${REDLINEDB_POSTGRES_PORT:-55432}`. The script waits for container health,
exports `REDLINEDB_POSTGRES_URL`, runs `beyond_sqlite_manifest` and
`beyond_postgres_reference`, then removes the container. Set
`REDLINEDB_POSTGRES_KEEP=1` to keep the local container for debugging.

This lane is separate from official parity. Its
`boolean_and_uuid_strict_storage_matches_postgres` case currently fails at
`CREATE TABLE`: the default SQLite execution mode rejects `BOOLEAN` in a
`STRICT` table before the PostgreSQL storage comparison. The published
v5.1.1 CLI has the same refusal; see the executable examples in
[known limitations](known-limitations.md).

`REDLINEDB_POSTGRES_URL` enables these optional Rust reference tests.
When both `REDLINEDB_POSTGRES_URL` and
`REDLINEDB_REQUIRE_POSTGRES_REFERENCE` are unset, oracle-dependent tests
return early, even though the Rust test runner reports a pass; those
comparisons are unmeasured.
`REDLINE_TESTING_POSTGRES_URL` is the separate variable used by the official
parity runner and CI service. Set that variable for official PostgreSQL
proof in `pr-ci`; it does not enable the optional Rust reference lane.

To reproduce the PR-side jankurai failure mode before pushing, commit the
candidate changes and run:

```
rtk scripts/ci-local.sh pr-gate
```

That command fetches `origin/main`, applies the same branch-freshness check as
`.github/workflows/jankurai.yml`, then runs `ops/ci/jankurai-staged-gate.sh`
with `BASE_REF=origin/main`.

To reproduce the complete PR CI surface locally, run:

```
rtk scripts/ci-local.sh pr-ci
```

That command runs the same shared dispatchers used by `.github/workflows/ci.yml`:
`CI_FAST_STAGE=preflight`, each `tests` matrix shard and
`CI_PARITY_STAGE=redline-testing-official`, then the component, integration,
package, security and audit dispatchers. It stops at the first failing local
job and preserves the underlying command output. `pr-ci` does not invoke
`scripts/guard-official-evidence.sh`; the official-evidence guard is a
separate lane. Without `REDLINE_TESTING_POSTGRES_URL`, local PostgreSQL
cases are skipped, so SQLite-only local proof is not PostgreSQL proof.

The parity stage ends with `ops/ci/check-report.sh`, which renders the SQLite
report in official mode (`--run-provenance`) and the PostgreSQL block through
`check-postgres`, both into scratch copies. Both refuse a run whose source tree
was dirty when it started: an uncommitted or untracked change under the source
input paths (`SOURCE_INPUT_PATHS` in
`subrepos/redline-testing/src/evidence/identity.rs`) means the measured
binaries are not the commit being pushed. `pr-ci`, and so the pre-push hook,
therefore need a clean checkout; commit, stash or move work in progress first.
Generated outputs (README, `benchmark-results/`, `assets/`) and ignored build
output do not count.

To run local mirrors for the broader PR workflow set, including dependency
review, branch freshness, staged jankurai gate, and the input-boundary FFI
cross-check, run:

```
rtk scripts/ci-local.sh all
```

## Publication and review

`main` on `neverhuman/redline` (repository id `1390165945`, named in
`ops/release/authority.env`) must carry this protection, which an admin
applies with `bash ops/release/main-protection.sh apply`:

| Setting | Value |
| --- | --- |
| Required status check | `RedlineDB/required`, strict: the head must be up to date with `main` |
| Approving reviews | 1, dismissed when a new commit is pushed |
| History | linear; force-push and deletion refused; enforced for admins |
| Merge method | rebase only; squash merges and merge commits are disabled |

`bash ops/release/main-protection.sh check` compares the live settings with this
table (read-only; any GitHub login that can read the protection) and lists every
difference. A repository admin applies the table with
`bash ops/release/main-protection.sh apply`. `ops/ci/tests/main-protection.sh`
tests both against a stub `gh`.

A pull request needs an eligible reviewer: a login that did not open the pull
request and did not author or commit any commit on it. Rewriting author or
committer so a login becomes eligible is not a review. A commit whose author has
no GitHub login (a bot address) is attributed to whoever produced it before any
review. The logins, credentials and host paths the maintainers use for the two
roles are operator configuration and are not kept in this repository.

1. `just pr-ci` exits 0, then one push of that head.
2. The writer opens the pull request:

```sh
gh pr create --repo neverhuman/redline --base main --head <branch> \
  --title "<subject>" --body-file <body-file>
```

3. The reviewer checks eligibility, then approves the full head SHA after the push:

```sh
gh pr view <PR> --repo neverhuman/redline --json author,headRefOid \
  --jq '{opened_by: .author.login, head: .headRefOid}'
gh api --paginate repos/neverhuman/redline/pulls/<PR>/commits \
  --jq '.[] | {sha, author: .author.login, committer: .committer.login}'
gh api --method POST repos/neverhuman/redline/pulls/<PR>/reviews \
  -f commit_id=<full sha> -f event=APPROVE -F body=@<review-file>
```

4. The writer merges after `RedlineDB/required` is success on that same SHA:

```sh
gh pr merge <PR> --repo neverhuman/redline \
  --rebase --delete-branch --match-head-commit <full sha>
```

Rebase is the one merge method: it keeps the reviewed commits as they are on a
linear `main`. The parity report bot (`.github/workflows/report-merge.yml`)
merges its report pull requests the same way.

## Budgets and stop conditions

The limits in [`.jankurai/cost-budget.toml`](../.jankurai/cost-budget.toml)
are authored audit policy. They are not a runtime enforcement guarantee
for the published v5.1.1 benchmark harness. The historical policy names
`REDLINEDB_BENCH_KILL` and `kill_receipt.json`, but that release implements
neither the environment-variable polling nor the receipt writer. Setting
the variable does not stop a run, and exporting an environment variable in
another shell cannot change an already-running process.

The policy also names dependency quotas (`max_advisory_count` and the license
allowlist), a CI concurrency ceiling, and per-workload wall-clock, disk and
syscall budgets. A declared spend cap is an operator limit, not proof that
the harness enforces it. Record the selected limits and stop conditions in
the run receipt, check available storage before starting, and monitor growth
while the run is active. External `timeout` or Ctrl-C is the actual kill
switch; preserve the interrupted run as rejected evidence. The dependency
security lanes enforce their own advisory and license rules through
`cargo audit` and `cargo deny`.

Set a shell timeout before starting your own run, or interrupt that run
with Ctrl-C. Preserve its exit status, raw logs and any partial output; do
not label interrupted or reduced work publishable. Never stop another
agent's job. The release scoreboard script has its own load, runner-job,
completeness, correctness and SQLite-control checks; its bundle summary
must pass before rendering published numbers.

## Structured errors and repair receipts

The optional structured error helper is defined at
`crates/domain/src/error.rs::DomainError`. Every `DomainError`
carries six fields:

- `purpose` — a `module.subsystem.event` triple naming where the
  failure occurred (e.g. `kernel.storage.invalid_checksum`).
- `reason` — a one-sentence human explanation suitable for logs.
- `common_fixes` — a `&'static [&'static str]` of grep-able repair
  hints the next agent can scan without rereading the source.
- `docs_url` — the in-repo doc path that explains the dimension this
  failure belongs to (usually `docs/audit-rubric.md#<dimension>`).
- `repair_hint` — the specific proof lane to rerun.
- `source` — the underlying `Box<dyn Error + Send + Sync>` so the
  full causal chain stays attached.

This is not the universal error representation for the kernel, SQL or FFI.
The explicit conversion example lives at
`crates/kernel/src/error.rs::Error::into_domain` for the
`InvalidChecksum` variant. The unit tests in both crates (`cargo test
-p redlinedb-domain` and `cargo test -p redlinedb-kernel`) assert the
field shape so renames stay safe.

Authoring a new escalation:

1. Add the kernel/SQL/FFI variant to the relevant `Error` enum as
   usual.
2. Extend that crate's `into_domain` (or write one if it does not
   exist) to wrap the variant via
   `DomainError::new(...).with_source(self)`.
3. Add a unit test that asserts each of the six fields and the
   source chain.
4. Link the new failure under the relevant dimension in
   `docs/audit-rubric.md`.

A `proof-receipt.md` template lives at
`.jankurai/proof-receipt-template.md`; use it to record the lane name,
seed, raw-log path, and exit code for any non-trivial repair.

## Release readiness — launch-gate evidence

Test evidence rolls into the release-readiness gate documented in
[`docs/release.md`](release.md). The launch gates that every
tagged release must satisfy:

- **Security** — `just security` (cargo audit, cargo deny,
  gitleaks) green; the `security` job in
  `.github/workflows/ci.yml` blocks the PR otherwise.
  `bash ops/ci/security-receipt.sh` on the candidate writes
  `target/security/receipt.json`; see
  [`docs/security-scans.md`](security-scans.md).
- **Backups** — kernel `Engine::backup` integration test green
  (`cargo test -p redlinedb-kernel backup`); restore round-trip
  proven by the failpoint matrix lane.
- **Qualification monitoring and evidence custody** — check every required
  workflow job's conclusion and preserve its raw logs. The release acceptance manifest binds the
  security, audit, parity and durability receipt digests. Downloaded receipts
  are checked in the qualification custody directory;
  [RELEASING.md](RELEASING.md) describes those checks.
  This verifies the qualification run; it does not certify an application's
  deployment monitoring.
- **Rollback** — releases are never deleted or replaced: a defective
  release is superseded by a corrected one (`docs/release.md`, "Evidence and
  rollback"), and nothing is published to crates.io; `release-bad-behavior`
  lane in `.jankurai/proof-lanes.toml`.
- **Abuse controls** — FFI input boundary tests
  (`cargo test -p redlinedb-ffi shell`) plus the authz matrix lane
  cover misuse of the C ABI from untrusted callers.

This section indexes the release gate (HLT-025). Individual passing tests do
not establish a qualified release; the acceptance manifest and downloaded
receipts must agree. The release-process steps live in `docs/release.md`;
this section is the testing-side index.
