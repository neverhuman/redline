# Coverage ledger

Copied from the policy file and from a local `sqlite_parity` run on this branch. Nothing rewrites this page on build. When the policy or a new official summary is committed, edit this page in the same commit so the counts stay tied to those files.

## SQLite

From `benchmark-results/sqlite-parity/latest/summary.json`:

| Field | Value |
| --- | --- |
| Suite | sqlite_parity |
| Total | 2445 |
| Passed, this branch, 1 repetition | 2445 |
| Failed, this branch | 0 |
| Skipped, this branch | 0 |
| Committed latency report | 2441 passed, 0 failed, 4 skipped, 3 repetitions, 1 warmup |

The README block under `sqlite-parity-report:begin` still repeats the latency report. The badge above it that says 2374/2445 is older. Cases 93–96 passed in the branch run: `fts5`, highlight, `rtree`, and `dbstat`.

## Postgres

Corpus size, from `subrepos/redline-testing/corpus/beyond_sqlite/generated_manifest.json`: **265** cases.

Open failures, from `metadata/beyond_sqlite/postgres-regression.json`: **0** cases.

Policy reason, copied from that file: Text search and the nine declared rejections agree with PostgreSQL 16.15. No open failures remain.

Oracle settings the gate requires: `160015|C|C|UTC`.

The generated README paragraph between `POSTGRES_PARITY_START` and `POSTGRES_PARITY_END` reports 265 passed, 0 failed, 0 skipped. Nine of those passes are the agreed rejections below. Both engines exit 3, and the case names the RedlineDB stderr text.

| Case | Name | Declared text |
| ---: | --- | --- |
| 20021 | `ENUM_REJECTS_BAD_VALUE` | `invalid input value for enum color: "purple"` |
| 20023 | `DOMAIN_REJECTS_NEG` | `value for domain positive_int2 violates check constraint "positive_int2_check"` |
| 20103 | `MERGE_NOT_MATCHED_BY_SOURCE_PG17` | `syntax error at or near "BY"` |
| 20111 | `DISTINCT_ON_REQUIRES_ORDER_BY` | `SELECT DISTINCT ON expressions must match initial ORDER BY expressions` |
| 20308 | `PLPGSQL_RAISE_EXCEPTION` | `ERROR: boom` |
| 20340 | `VECTOR_EXTENSION_PROBE_UNAVAILABLE` | `extension "vector" is not available` |
| 20418 | `LOGICAL_SLOT_REJECTS_WAL_LEVEL_REPLICA` | `logical decoding requires wal_level >= logical` |
| 20429 | `LOGICAL_SLOT_PEEK_REQUIRES_SLOT` | `logical decoding requires wal_level >= logical` |
| 20438 | `LISTEN_INVALID_CHANNEL_NAME_ALL` | `syntax error at or near "ALL"` |

A case leaves the open list when a raw beyond-SQLite JSONL record shows `status` passed and the policy file drops the id. A shared non-zero exit counts as that pass only after `expected_target_stderr_contains` is set and the target produces that text.

## Where the other chapters point

| Question | File |
| --- | --- |
| Feature-by-feature SQLite map | `docs/sqlite-parity.md` |
| Why some Postgres cases are deferred in the skip list | `docs/beyond-postgres-skips.md` |
| RQL document shape | `docs/rql.md` |
| Crate boundaries | `docs/boundaries.md` |
| Installer details that drift | `docs/install_redlinedb.md` is older than [Start here](02-start-here.md) |

