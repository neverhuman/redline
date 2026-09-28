# Changelog

This project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html). The
release surface is the signed runner tarball; see [docs/release.md](docs/release.md)
for the publish + attestation flow.

## [Unreleased]

### Changed

- Bound Jain release packaging to the Cargo product version through the Rust
  `xtask validate-release-tag` gate and the authorized immutable corrective tag
  `redline-testing-v1.0.1-jain.1`; the existing `.0` tag remains unchanged.
- Removed the remaining Python CI helper in favor of the tested Rust xtask
  implementation.
- Latency is reported as the per-case RedlineDB/SQLite ratio with no 3 ms
  SQLite reference floor, in both `run` and `report`. `ranked.csv` replaces
  `improvement_pct` with `latency_ratio`, `gap_pct_raw` and `below_resolution`
  and is sorted by ratio, slowest first; `summary.json` records
  `ranked_schema: redline-testing-ranked-v2` and
  `measurement_boundary: cli_case_wall_time`. A zero median is an error.
- The `sqlite_parity` README badge and report block describe the measured
  SQL/CLI corpus, not SQLite. The badge reads `SQLite SQL/CLI corpus` with
  `passed/total · failed · skipped · oracle`; it is green only for a run with
  official evidence, no failures and no skips, red with failures, orange with
  skips, and grey `unqualified` without consistent official evidence. The
  block names the corpus and oracle, states that C ABI semantics, the file
  format and prepared-statement state are out of scope, and lists the
  declared deviations from `metadata/sqlite_parity/declared-deviations.json`
  that are in the run. It reads `corpus_sha256` and `oracle_build_stamp` from
  the run evidence when present and prints `unrecorded` otherwise.
- `xtask generate` and `xtask ship-gate` default `--sqlite-bin` to the pinned
  sqlite3 3.53.1 reference that `scripts/sqlite/build-reference.sh` builds
  (`$REDLINEDB_SQLITE_REFERENCE_PREFIX/bin/sqlite3` when that is set), and
  refuse a shell with no `.sqlite-reference-sha3` build stamp or of another
  release. They used to run `sqlite3` from PATH (Ubuntu's 3.45.1), so the
  corpus was blessed against a different build from the oracle it is scored
  against.
- The `sqlite_parity` corpus is re-blessed against the pinned 3.53.1 shell:
  141 cases declared 3.45.1 behaviour (127 shard cases, 14 manifest cases).
  The `gen_*` shards are regenerated; hand-written shard cases take the pinned
  shell's output; the manifest cases are migrated by hand (listed in
  RedlineDB's `docs/sqlite-parity.md`). The `soundex()` generator rows now
  record the pinned build's `no such function: soundex` rejection, and
  `generate` refuses a declared fragment the capture does not contain.
  `ship-gate` checks the pinned manifest as well as the shards by default.
- `metadata/sqlite_parity/declared-deviations.json` is schema v2: each entry
  has a `kind`. `stand_in` covers the existing entries. `shared_rejection`
  covers 11437-11439 (`soundex()`) and 00219/00220 (`UPDATE`/`DELETE ...
  LIMIT`), which the pinned build rejects. `oracle_build` covers 10546, which
  was written for a build without `median()`. The report lists each kind
  under its own heading, and a unit test requires every shared rejection
  to declare a non-zero exit and a stderr fragment.
- The runner enforces each case's declared contract on the reference before it
  compares engines (`validate_expected` in `sqlite_parity/runner.rs`). The
  reference must exit normally with `expected_exit`, print every declared
  stdout, stderr and combined fragment, and match `expected_stdout` exactly
  (normalized) when the case compares stdout. A violation fails the case as
  `reference_contract_failure`, even when both engines agree. A child killed
  by a signal (no exit code) never passes; two such children used to compare
  equal. Raw records gain `verdict_reason` (`passed`, `skipped`,
  `reference_contract_failure`, `differential_mismatch`) and `stage`
  (`selection`, `reference_contract`, `differential`). `rql_phase1` uses the
  same verdict. A catalog test requires every case with a non-zero
  `expected_exit` to declare a stderr or combined fragment.
- The target is held to the same declared contract before the shells are
  compared: a wrong exit code, a missing declared fragment or a signal is
  `target_semantic_failure` (stage `target_contract`), whether or not the case
  compares stdout. Case 10547 with a target that prints `no such table: t`
  fails.
- The differential compares raw bytes. `EngineOutput` stdout and stderr are
  `Vec<u8>`; they are hashed and written to failure artifacts as captured, and
  nothing is decoded, trimmed or folded. Cases gain `comparison_mode`
  (`cli_bytes_exact`, the default, or `cli_text_lf`, which reads CRLF as LF),
  `ignore_line_prefixes` (replacing the hardcoded case-208 filter) and
  `stdout_uncompared_reason`, required exactly when `compare_stdout` is false.
  Raw records carry `normalization_policy` (`sqlite-parity-compare-v2`) and
  `comparison_mode`. Declared fragments and `expected_stdout` are still matched
  on normalized text.
- The runner no longer aborts on a failed case. `run --sqlite-known-failures
  <path>` gates `sqlite_parity` and `memory` on a baseline
  (`redline-testing-sqlite-known-failures-v1`): after every suite ran and the
  official evidence is written, a failure the baseline does not list, a listed
  case that passed or was skipped, and a listed case that failed with another
  verdict all fail the run. `rql_phase1` has no baseline, so any failure there
  is fatal. Summaries, `summary.json` and `official-evidence.json` list failed
  (and skipped) case ids, and the official evidence records the baseline's
  sha256.
- Case execution is bounded (SQ-09). Each engine run gets its own process
  group, a stdin writer thread and stdout/stderr reader threads capped at
  `--max-output-bytes` (default 16 MiB), and a deadline, `--case-timeout-ms`
  (default 60000); a timeout or a byte past the cap kills the whole group, and
  what a run leaves in its group is killed after it exits. A run that timed
  out, passed the cap or could not start fails its sample as
  `execution_failure` (stage `execution`) instead of hanging, exhausting
  memory or aborting the run; raw records gain `execution_outcome`,
  `reference_execution_outcome` and `target_execution_outcome`. Records are
  streamed through one writer thread as each case completes, and a finished
  suite writes `<raw>.complete.json`; `official-evidence.json` declares and
  hashes it (`completion_path`) and records `case_timeout_ms` and
  `max_output_bytes`. `run --case-id` narrows one suite for diagnosis.
- Skips follow an exact policy, not a budget (SQ-05).
  `corpus/sqlite_parity/scope-policy.json`
  (`redline-testing-sqlite-scope-policy-v1`, compiled in) lists every case a
  `sqlite_parity`, `memory` or `rql_phase1` run may skip, with `suite`,
  `case_id`, `name`, `kind` (`target_capability` or `rql_rewrite`), `reason`,
  `owner` and `expiry`; it holds the 202 `rql_phase1` rewrite gaps. A gap
  the policy does not list fails the case as `target_unsupported`; a
  reference shell without a declared capability fails it as
  `reference_capability_missing`; a listed case that runs fails the run.
  Skipped records carry `policy_exception_id`, failed selections a `not_run`
  placeholder. Unknown capability tokens and case statuses are errors (case
  10600's `REGEXP` is now the known token `regexp`, probed on both shells),
  and a capability probe that cannot run, times out or floods is an error
  instead of an absent capability; probes are bounded like cases.
  `run --official` requires `--suite all` and `--sqlite-known-failures`,
  refuses `--case-id`, `REDLINE_TESTING_PINNED_ONLY` and expired exceptions,
  and writes `run_mode: official`; other runs write `run_mode: diagnostic`.
  `official-evidence.json` records `sqlite_scope_policy` (schema, path,
  sha256) and each suite's `skipped_case_ids`.
- Case verdicts are disjoint and complete (SQ-04). `report` reduced raw
  records to overlapping sets: a case with a failed warmup and passing
  measured samples counted as both passed and failed, repetitions were
  checked over the whole run rather than per case, duplicates and missing
  cases went unnoticed, and `repetitions` defaulted to the number of measured
  samples. `report/verdicts.rs` `reduce_sqlite_verdicts` now requires every
  sample identity (case, role, repetition) once, each executed case to have
  exactly the run's warmups and repetitions `1..=R`, a skipped or rejected
  case to be one placeholder, and the cases to be exactly the suite's
  compiled-in manifest; a case with any failed record is failed, a policy
  skip skipped, and only a clean case passed. A report from official
  evidence must state `--expected-warmup` and `--expected-repetitions`. New
  `check-sqlite --official-evidence <run>/official-evidence.json --output
  <file>` applies the same reduction to `sqlite_parity`, `memory` and
  `rql_phase1`, checks the runner's counts and failed and skipped ids
  against it, and writes `sqlite-qualification.json`
  (`redline-testing-sqlite-qualification-v1`) with every case id by verdict,
  the raw and manifest hashes, the sample plan and the runner's hash.
- `run --order sqlite-first|target-first|alternate` chooses which engine runs
  first in each sample. The default, `sqlite-first`, is the order every run
  used before, so the correctness lane is unchanged. `alternate` starts even
  sample indexes (warmups included) with the reference and odd ones with the
  target. Every raw record carries `measurement_order` and `first_engine`
  (`reference` or `target`; `null` on a placeholder), and each suite's
  `manifest.json` carries `measurement_order`.

### Removed

- The placeholder report cards and their flags: `--ksloc-plot` (a hardcoded
  `redline-testing, LOC 1`), `--jankurai-score-plot`, `--code-shape-plot` and
  `--jankurai-comparison-plot` (case counts under Jankurai titles), and
  `--jankurai-comparison` (a raw JSON dump). `report` no longer writes
  `ksloc.csv` or `paper-data-loc-comparison.csv`, and it deletes the retired
  `sqlite-parity-metrics` and `sqlite-jankurai-breakdown` README blocks
  instead of appending them.

## [1.0.1] - 2026-05-26

### Fixed

- Restored the GitHub CI mirror and GitHub release workflow alongside the
  GitLab validation pipeline.
- Re-aligned the release docs, ownership maps, and CI guardrails so the
  public release host and local proof surface are explicit again.

## [1.0.0] - 2026-05-25

### Added — exhaustive corpus expansion

- **24 hand-authored SQLite-parity shards** under `corpus/sqlite_parity/cases/`
  (630 cases, IDs 10001–10630, P0 priority). Categories: NULL semantics, NULL
  ordering, aggregate-NULL, autoincrement, strict tables, CLI output mode, CLI
  dot-command, CLI option, PRAGMA P0, foreign keys, transactions, UPSERT,
  RETURNING, ATTACH, schema introspection, ALTER, recursive CTE, JOIN, compound,
  subquery, aggregate-advanced, error messages, pattern, BLOB.
- **8 xtask matrix-generated shards** under `corpus/sqlite_parity/cases/gen_*`
  (688 cases, IDs 11000–12037). Generators in `xtask/src/generators.rs`:
  `math` (109), `cast` (45), `affinity` (64), `string` (52), `datetime` (240),
  `json_path` (67), `window` (73), `pragma_sweep` (38). `cargo run -p xtask --
  generate --check` is the drift guard.
- **12 beyond-SQLite oracle shards** under `corpus/beyond_sqlite/generated_manifest.json`
  (265 cases, IDs 20001–20444) covering all 12 ranked feature areas in
  `metadata/beyond_sqlite/features.json`. Validated via psql ↔ psql self-compare
  in `src/beyond_sqlite/oracle.rs`; 253 passed, 12 graceful skips, 0 failures.

### Added — runner / infrastructure

- `xtask` workspace member with `generate` and `ship-gate` subcommands. Not
  shipped in the release tarball.
- `build.rs` enumerates `corpus/sqlite_parity/cases/*.json` at compile time so
  `include_str!` picks up new shards automatically.
- `src/beyond_sqlite/` module split: `mod.rs`, `case.rs`, `engine.rs`,
  `normalize.rs`, `oracle.rs`, `taxonomy.rs`. Postgres resolver returns
  `Configured(_) | Unavailable(_)`; SQLite-parity suite is mathematically
  incapable of depending on Postgres.
- Extended capability enum (`fts5`, `rtree`, `jsonb`, `math1`, `generate_series`,
  `json_pretty`, `jsonb_array_insert`) + probes in
  `src/sqlite_parity/engine.rs`.
- 5 new Rust integration tests under `tests/`: shard schema, beyond manifest,
  capability probe, release-manifest integrity, runner JSONL invariants.
- `scripts/release-package.sh` replaces the monoline justfile recipe — every
  file under `dist/<package>/` is glob-hashed into `artifact_hashes`.
- `justfile`: new `check`, `test`, `verify`, `generate-check`, `ship-gate`
  recipes alongside `pr-ci` and `release-local`.
- `.github/workflows/ci.yml`: optional `beyond-postgres` job (push +
  workflow_dispatch + `exhaustive` branch) that spins up a postgres:16
  service container and runs the oracle path. The default `pr-ci` job stays
  Postgres-free, preserving the SQLite-parity invariant.

### Changed

- `.jankurai/audit-policy.toml` — adopted with `mode = "advisory"`, scan
  exclusions for `tests/` integration files and `xtask/`. Mirror in
  `agent/audit-policy.toml`.
- `Cargo.toml` — workspace `[".", "xtask"]`, `default-members = ["."]`,
  resolver 3, `[features] pg-embedded` (declaration only; dep not yet wired).
- README.md / AGENTS.md — document the corpus development flow, ship contract,
  oracle gating, and new tarball contents.

### Notes

- The pinned upstream manifest `corpus/sqlite_parity/generated_manifest.json`
  (1,127 cases) stays byte-identical.
- New cases start at ID 10001 (parity) / 20001 (beyond), reserving 1128–9999
  for upstream growth.
- Ship contract: a case lands on `main` only after passing reference
  self-compare (`sqlite3 ↔ sqlite3` for parity, `psql ↔ psql` for beyond).

## [0.1.3] - 2026-05-24

Baseline release before the exhaustive expansion. See git history for the
prior runner + report-gate work.

[1.0.0]: https://github.com/neverhuman/redline-testing/compare/v0.1.3...v1.0.0
[1.0.1]: https://github.com/neverhuman/redline-testing/compare/v1.0.0...v1.0.1
[0.1.3]: https://github.com/neverhuman/redline-testing/releases/tag/v0.1.3
