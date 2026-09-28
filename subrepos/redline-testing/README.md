> Source, reviews, CI and releases: [neverhuman/RedlineDB](https://github.com/neverhuman/RedlineDB).
> This component lives at `subrepos/redline-testing` in the canonical checkout.
> Follow the [root release process](../../docs/release.md); historical component release examples below are archival.

# redline-testing

Primary CI runs on GitHub via [.github/workflows/ci.yml](.github/workflows/ci.yml).
Public release artifacts are published on GitHub via [.github/workflows/release.yml](.github/workflows/release.yml).
<!-- jankurai-score-badge:begin -->
[![Jankurai score: 38/100 advisory](https://img.shields.io/badge/jankurai-38%2F100%20advisory-red)](agent/repo-score.json)
<!-- jankurai-score-badge:end -->

Conformance, benchmark, and SQLite-parity test harness for **any SQLite-compatible database**.
Point `--target-bin` at your binary; the runner does the rest.

Suites:

| Suite | What it tests | Cases | Postgres? |
|---|---|---|---|
| `sqlite_parity` | SQL + CLI conformance vs SQLite reference | 2,445 | No |
| `memory` | Same corpus with Linux `/proc` RSS sampling | 2,445 | No |
| `rql_phase1` | Redline Query Language phase 1 conformance | 1,385 | No |
| `beyond_sqlite` | PostgreSQL-class features, oracle-validated | 265 | Optional |
| `all` | All suites + `official-evidence.json` hash bundle | 4,095+ | Optional |

---

## Install

Download the pre-built binary from [GitHub Releases](https://github.com/neverhuman/redline-testing/releases/latest):

```bash
curl -fsSL \
  https://github.com/neverhuman/redline-testing/releases/latest/download/redline-testing-1.0.1-linux-x86_64.tar.gz \
  | tar -xz
./redline-testing-1.0.1-linux-x86_64/bin/redline-testing --version
```

Each release ships a `.sha256` sidecar and a Sigstore/SLSA build-provenance
attestation. Verify the tarball hash before you run:

```bash
sha256sum -c redline-testing-1.0.1-linux-x86_64.tar.gz.sha256
```

You can also verify the attestation with GitHub CLI:

```bash
gh attestation verify redline-testing-1.0.1-linux-x86_64.tar.gz \
  --repo neverhuman/redline-testing
```

---

## Quick start

Replace `your-db` with any SQLite-compatible binary — not just RedlineDB:

```bash
redline-testing run \
  --suite sqlite_parity \
  --target-bin /path/to/your-db \
  --sqlite-bin /usr/bin/sqlite3 \
  --output results.jsonl
```

The runner compares `your-db` against the SQLite CLI reference using
`corpus/sqlite_parity/` (1,127 pinned upstream cases + 1,318 extended cases).
It writes JSONL records on stdout/file and exits non-zero if any case fails.

---

## Develop

Setup and validate are one command each:

```bash
bash scripts/setup.sh    # install the pinned toolchain, fetch deps, build
bash ops/ci/pr-ci.sh     # the single validate command (fmt, check, test, package, security)
```

Other lanes (also runnable via `just`):

```bash
bash ops/ci/security.sh  # gitleaks + cargo-audit + cargo-deny + zizmor + SBOM
bash ops/ci/jankurai.sh  # jankurai tool-suite evidence -> target/jankurai/**
```

Agent-readable docs: [docs/architecture.md](docs/architecture.md),
[docs/boundaries.md](docs/boundaries.md), [docs/testing.md](docs/testing.md),
[docs/operations.md](docs/operations.md), and per-cell `ops/AGENTS.md`.

---

## Run

```bash
redline-testing run \
  --suite sqlite_parity \     # sqlite_parity | memory | rql_phase1 | beyond_sqlite | all
  --target-bin /path/to/db \
  --sqlite-bin /path/to/sqlite3 \
  --workers auto \            # accepted; execution is serial in this release
  --tmp-root auto \
  --output raw.jsonl \
  --repetitions 1 \
  --warmup 0 \
  --progress auto \
  --case-timeout-ms 60000 \   # kill an engine run's process group after this
  --max-output-bytes 16777216 \ # ... or once it writes more to stdout or stderr
  --memory-samples            # Linux /proc RSS sampling (memory suite)
```

`--case-id <ID>` (repeatable) narrows one SQLite-shell suite to the named
cases for diagnosis; such a run is never official evidence. `--official`
marks the RedlineDB official lane: it requires `--suite all` and
`--sqlite-known-failures`, and refuses `--case-id`,
`REDLINE_TESTING_PINNED_ONLY` and an expired scope-policy exception. Only its
evidence (`run_mode: official`) is publishable.

### Environment variables

| Variable | Default | Effect |
|---|---|---|
| `REDLINE_TESTING_PINNED_ONLY` | unset | Set to `1` to run only the 1,127 pinned upstream cases (skip extended shards); `run --official` refuses it whatever its value |
| `REDLINE_TESTING_POSTGRES_URL` | unset | PostgreSQL DSN for the `beyond_sqlite` oracle (e.g. `postgresql://localhost/postgres`) |

### Output fields

`raw.jsonl` follows the report-parser contract:

```
case_id  name  case_file  priority  profile  category  sample_role  repetition_index
sqlite_version  reference_engine  target_engine
reference_executable_path  target_executable_path
reference_executable_sha256  target_executable_sha256
reference_version  target_version
status  reference_elapsed_ns  target_elapsed_ns
```

When `--memory-samples` is set on Linux, records also include:
`memory_status  reference_peak_rss_kb  target_peak_rss_kb`

---

## Suites

### `sqlite_parity`

Compares your target binary against the SQLite CLI reference for 2,445 cases:

- **1,127 pinned upstream cases** (`corpus/sqlite_parity/generated_manifest.json`, IDs 1–1127, read-only)
- **630 hand-authored cases** (`corpus/sqlite_parity/cases/`, IDs 10001–10630, P0 priority) —
  NULL semantics, ordering, aggregates, autoincrement, strict tables, CLI output modes,
  dot-commands, options, PRAGMA P0, foreign keys, transactions, UPSERT, RETURNING, ATTACH,
  schema introspection, ALTER, recursive CTEs, JOINs, compound queries, subqueries,
  aggregate-advanced, error messages, pattern matching, BLOB
- **688 matrix-generated cases** (`gen_*` shards, IDs 11000–12037) — math, cast, affinity,
  string, datetime, JSON path, window functions, pragma sweep

### `memory`

Same parity corpus with Linux `/proc` RSS sampling enabled. Writes:
`memory.raw.jsonl`, `memory-summary.json`, `memory-ranked.csv`,
`memory-manifest.json`, `memory-provenance.json`.
Falls back gracefully if `/proc` is unavailable (`memory_status: unavailable`).

### `rql_phase1`

Redline Query Language phase 1 corpus, exercised against the SQLite reference
CLI and the target binary. Writes:
`rql_phase1.raw.jsonl`, `rql-phase1-summary.json`, `rql-phase1-ranked.csv`,
`rql-phase1-manifest.json`, `rql-phase1-provenance.json`.

### `beyond_sqlite`

265 oracle-validated cases covering 12 PostgreSQL feature areas
(`metadata/beyond_sqlite/features.json`): advanced types, window functions,
CTEs, stored procedures, full-text search, JSON operators, geospatial,
materialized views, advisory locks, logical replication, LISTEN/NOTIFY, and MONEY arithmetic.

When `REDLINE_TESTING_POSTGRES_URL` is set (or the conventional fallback
`/dev/shm/redline-pg-sock:5433` is reachable), the runner executes a
`psql ↔ psql` oracle for each case. Without Postgres every oracle case emits
`status: skipped` with a diagnostic — the SQLite-parity suite is unaffected.

### `all`

Runs every suite and writes `all.jsonl`, `all-manifest.json`, all per-suite
artifacts, and `official-evidence.json` (schema `redline-testing-official-evidence-v1`).
The evidence bundle records the runner, target, SQLite reference, per-suite
totals, and SHA-256 hashes of all declared output files.

---

## Report generation

```bash
redline-testing report \
  --suite sqlite_parity \
  --input raw.jsonl \
  --official-evidence official-evidence.processed.json \
  --out-dir benchmark-results/sqlite-parity/latest \
  --readme README.md \
  --updated-date 2026-05-25
```

`--official-evidence` verifies that the input hash matches the recorded
suite hash before rendering. Omit it only with `--local-diagnostics` for
uncommitted local diagnostics.

---

## Release tarball contents

```
bin/redline-testing                               runner binary
release-manifest.json                             metadata + artifact_hashes (SHA-256 map)
corpus/sqlite_parity/generated_manifest.json      1,127 pinned cases
corpus/sqlite_parity/cases/*.json                 extended hand-authored + generated shards
corpus/beyond_sqlite/generated_manifest.json      265 oracle cases
metadata/beyond_sqlite/features.json              12-entry feature taxonomy
schemas/raw-record.schema.json
schemas/release-manifest.schema.json
templates/README.sqlite-parity.md
```

Tagged GitHub releases are built by [.github/workflows/release.yml](.github/workflows/release.yml),
which reruns `pr-ci`, packages the tarball via `just release-local`, attests
the tarball + `.sha256` + `release-manifest.json`, and publishes the assets
with `gh release create --verify-tag`. Jain corrective tags use the immutable
`redline-testing-v<product-version>-jain.<revision>` identity; this release
candidate is `redline-testing-v1.0.1-jain.1`.

---

## Development

```bash
# Run all 31 integration tests
cargo test --locked

# Full CI mirror (fmt + check + test + package)
just verify

# Build + package release tarball locally
just release-local

# Validate every corpus case against the pinned sqlite3 3.53.1 reference
# (ship-gate). Both xtask commands default --sqlite-bin to the shell
# `bash scripts/sqlite/build-reference.sh` builds in the RedlineDB checkout,
# and refuse any shell without its build stamp or of another release.
cargo run -p xtask --release -- ship-gate

# Detect drift in matrix-generated shards
cargo run -p xtask --release -- generate --check

# Sync the README score badge from agent/jankurai-badge.json
cargo run --locked --quiet -p xtask -- update-badge
```

### Adding test cases

**Hand-authored SQLite-parity shard:** create `corpus/sqlite_parity/cases/<N>_<name>.json`
with IDs starting at 10001+. Each case must pass `sqlite3 ↔ sqlite3` self-compare
against the pinned reference shell.
Run `cargo run -p xtask --release -- ship-gate` to validate before committing.

**Matrix-generated shard:** add a generator in [`xtask/src/generators.rs`](xtask/src/generators.rs)
and run `cargo run -p xtask -- generate` to emit `gen_*.json` shards.

**Beyond-SQLite case:** add to `corpus/beyond_sqlite/generated_manifest.json`
(IDs 20001+). Must pass `psql ↔ psql` self-compare with a local Postgres instance:

```bash
# Bring up a dev-time Postgres on /dev/shm
/usr/lib/postgresql/16/bin/initdb -D /dev/shm/redline-pg-data \
  --auth-local=trust --auth-host=trust -U ubuntu
/usr/lib/postgresql/16/bin/pg_ctl -D /dev/shm/redline-pg-data \
  -l /dev/shm/redline-pg.log \
  -o "-p 5433 -k /dev/shm/redline-pg-sock -h ''" start
cargo run --release -- run --suite beyond_sqlite \
  --target-bin sqlite3 --output /tmp/beyond.jsonl
```

### Corpus structure

```
corpus/
  sqlite_parity/
    generated_manifest.json   pinned upstream (IDs 1–1127, read-only)
    cases/                    extended shards (IDs 10001+)
      NN_name.json            hand-authored
      gen_NN_name.json        matrix-generated
  beyond_sqlite/
    generated_manifest.json   oracle cases (IDs 20001+)
metadata/
  beyond_sqlite/
    features.json             12-entry rank/owner feature taxonomy
    skip-list.toml            cases skipped pending engine implementation
```

**Ship contract:** a case ships iff its reference self-compare passes
(`sqlite3 ↔ sqlite3` for parity, `psql ↔ psql` for beyond). Failing cases
must be fixed or deleted — there is no quarantine path.

**Run verdict (`sqlite_parity`, `memory`, `rql_phase1`):** every sample first
checks the reference against the case's declared contract. The reference must
exit normally with `expected_exit`, print every declared stdout, stderr and
combined fragment, and, when the case compares stdout and declares it, print
exactly `expected_stdout` after normalization. A violation is a
`reference_contract_failure`, even when the target agrees with the reference.
The target is then held to the same declared exit code and fragments
(`target_semantic_failure`), and only then compared with the reference, byte for
byte (`differential_mismatch`; a case may declare `comparison_mode: cli_text_lf`
or `ignore_line_prefixes`). A child killed by a signal never passes. Each raw
record carries `verdict_reason`, `stage` and `normalization_policy`.

**Skips:** a case is skipped only when
`corpus/sqlite_parity/scope-policy.json` (compiled in) lists it for the suite,
with the gap (`target_capability` or `rql_rewrite`), a reason, an owner and an
expiry; its raw record names the exception (`policy_exception_id`). A gap the
policy does not list fails the case (`target_unsupported`), a reference shell
without a declared capability always fails it (`reference_capability_missing`),
and a listed case that runs fails the run. An unknown capability token, an
unknown case status, or a capability probe that cannot run, times out or
floods is an error, never a skip.

**Verdicts:** `report` and `check-sqlite` reduce a suite's raw records to one
verdict per case: failed when any record failed, skipped when its one record
is a policy skip, passed otherwise. Every executed case must have exactly the
run's warmups and measured repetitions once each, and the cases must be
exactly the suite's compiled-in manifest (`list --format json`), or the
reduction fails. `check-sqlite --official-evidence <run>/official-evidence.json
--output sqlite-qualification.json` records the result for the RedlineDB
evidence processor.

**Bounded cases:** every engine run starts in its own process group. Its stdin
is written by a separate thread and its stdout and stderr are drained while it
runs, at most `--max-output-bytes` each. At `--case-timeout-ms`, or at the
first byte past the cap, the whole group is killed; after any run, whatever is
left of its group is killed too. A sample whose run timed out, passed the cap
or could not start fails as `execution_failure` (stage `execution`); raw
records carry `execution_outcome` (`exited`, `signal`, `timeout`,
`output_limit`, `spawn_error`, `not_run`) and the same per engine. Records are
appended as each case completes, and a finished suite writes
`<raw>.complete.json` (the raw file's SHA-256 and record and case counts),
which the RedlineDB evidence processor and `report` require.

**Known failures:** `run --sqlite-known-failures <path>` publishes the
`sqlite_parity` and `memory` failures a baseline lists as failures and fails the
run, after its evidence is written, when the failed cases are not exactly the
listed ones. The official lane passes RedlineDB's
`metadata/sqlite_parity/known-failures.json`.

---

## License

Apache-2.0
