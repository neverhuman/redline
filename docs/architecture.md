# Architecture — RedlineDB

This is a one-screen agent-readable map of the RedlineDB workspace.
Pair with `.jankurai/owner-map.json` for ownership, `.jankurai/proof-lanes.toml`
for how to rerun proofs, and `docs/audit-rubric.md` for the dimension
mapping the audit scores.

## Crate map

| Crate                       | Path                  | Owner                         | Role                                                                 |
|-----------------------------|-----------------------|-------------------------------|----------------------------------------------------------------------|
| `redlinedb-domain`          | `crates/domain/`      | `storage-and-catalog`         | Policy-free cross-crate types (typed `DomainError`).                 |
| `redlinedb-kernel`          | `crates/kernel/`      | `storage-and-catalog`         | Pages, WAL, MVCC, catalogs, integrity, vector, JSONB.                |
| `redlinedb-sql`             | `crates/sql/`         | `sql-parser-planner-executor` | Parser, planner, executor, vectorized exec, dialect surfaces.        |
| `redlinedb`                 | `crates/redlinedb/`   | `public-rust-facade`          | Rust facade (Database, Connection, backup); unstable, see `docs/api-stability.md`. |
| `redlinedb-ffi`             | `crates/ffi/`         | `c-abi`                       | SQLite-shaped C ABI (`sqlite3_api`) plus the public C header.        |
| `redlinedb-cli`             | `crates/cli/`         | `cli-shell`                   | Command-line shell and admin commands (backup/restore).              |
| `redlinedb-server`          | `crates/server/`      | `framed-server`               | Network-facing framed server.                                        |
| `redlinedb-bench`           | `crates/bench/`       | `bench-harness`               | Certify, compat, recovery-matrix, failpoint, OLTP-gap workloads.     |

The dependency graph is a strict DAG: `domain → kernel → sql →
redlinedb → {ffi, cli, server}`, with `bench` reaching into the
workspace as a top-level consumer for measurement only. Nothing
under `crates/` depends on `crates/bench`.

## Layering rule

Higher layers may depend on lower layers; lower layers must not
reach upward. The audit enforces this via `agent/boundaries.toml`
and `docs/boundaries.md`. Domain types (`DomainError`,
`storage::PageId`, vector types) live in the lowest layer so every
layer above can produce structured failures without a backward
dependency.

## On-disk surfaces

- Page file: `crates/kernel/src/storage/page_file.rs`. Checksummed
  per page; corruption produces `Error::InvalidChecksum`. Callers can
  explicitly attach repair context through `Error::into_domain`.
- WAL: `crates/kernel/src/wal/`. Group commit lanes, semantic
  combiner, archive/retention.
- Catalog: `crates/kernel/src/catalog/`.
- Vector indexes: `crates/kernel/src/vector/{flat,hnsw,diskann}/`.
- JSONB: `crates/kernel/src/json/`.

## Build, test, repair

- Build: `rtk cargo build --workspace`.
- Default proof: `just fast` (fmt + file-size + check + test).
- Wider proof: `just pr-ci`, `just security`, lane-specific commands
  in `.jankurai/proof-lanes.toml`.
- Failure repair: preserve the actual typed error and failing command.
  When an explicit `DomainError` conversion supplied `docs_url` and
  `repair_hint`, use those fields to select the next proof lane.

## Where to start (agent router)

1. `AGENTS.md` (root) for the rules.
2. `.jankurai/owner-map.json` for who owns what.
3. `.jankurai/proof-lanes.toml` for how to rerun.
4. `docs/audit-rubric.md` for dimension-to-evidence mapping.
5. `docs/boundaries.md` for cross-crate edges.
6. `docs/language-bad-behavior.md` for the detector terms.
7. `docs/testing.md` for the proof-lane index and repair-receipt
   protocol.
