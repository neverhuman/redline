# Architecture

`redline-testing` is a single-crate Rust CLI plus an `xtask` dev tool. It
compares a target database CLI against a reference CLI by running shared SQL /
CLI cases through both and diffing normalized output.

## Layers

```
src/cli/              clap entry layer — parses args, routes to suites, renders
src/report/           report rendering + official-evidence bundle (over JSONL)
src/evidence.rs       evidence bundle hashing + serialization
src/sqlite_parity/    SQLite-parity suite
  ├── engine.rs       subprocess SQL driver (the data-access / adapter seam)
  ├── bounded.rs      process-group, deadline and output-cap bounds per run
  ├── runner.rs       case scheduling + compare loop
  ├── record_sink.rs  streamed raw records + completion marker
  ├── rql_phase1.rs   Redline Query Language phase-1 SQL→JSON lowering
  ├── catalog.rs      case catalog + capability gating
  ├── case.rs         case model
  ├── memory.rs       /proc RSS sampling
  └── normalize.rs    output normalization
src/beyond_sqlite/    PostgreSQL-class oracle suite
  ├── engine.rs       subprocess psql driver (adapter seam)
  ├── oracle.rs       psql self-comparison and target execution
  ├── gate.rs         artifact identities, reference pin and regression policy
  │   └── gate/cases.rs  complete, unique, executed case coverage
  └── taxonomy.rs     legacy feature metadata and execution artifacts
xtask/                dev-only corpus, badge, and receipt tooling (not shipped)
```

## Data-access boundary

There is **no in-process database driver**. Every "DB access" is a subprocess
invocation of an external `sqlite3` / `psql` shell. A SQLite-shell case run is
bounded by `sqlite_parity/bounded.rs`: its own process group, a deadline, an
output cap, and a group kill on expiry and after exit. The
`*/engine.rs` + `runner.rs` + `oracle.rs` modules are the data-access / adapter
seam; the reference CLI name is centralized as
`sqlite_parity::REFERENCE_CLI_BIN`. See [`docs/boundaries.md`](boundaries.md).

## Output contract

The runner emits JSONL raw records (`schemas/raw-record.schema.json`) and a
release manifest (`schemas/release-manifest.schema.json`). Root RedlineDB CI builds this included runner at the tested source commit;
historical reports can explicitly select a pinned release tarball. JSONL field compatibility is a hard contract. Generated
zones are declared in [`.jankurai/generated-zones.toml`](../.jankurai/generated-zones.toml)
and must not be hand-edited.

## Error surface

`src/exceptions.rs` defines the typed `HarnessError` surface. Each variant
exposes `purpose`, `reason`, `common_fixes`, `docs_url`, and `repair_hint` so a
failure routes the next agent straight to a local rerun.
