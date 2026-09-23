# Coverage ledger

Copied from the files named below, at commit `f8a0b7c2d336677d979dd504325a1e921ad4f73c`. Nothing rewrites this page on build. When the policy or the SQLite summary changes, edit this page in the same commit so the counts stay tied to those files.

## SQLite

From `benchmark-results/sqlite-parity/latest/summary.json`:

| Field | Value |
| --- | --- |
| Suite | sqlite_parity |
| Total | 2445 |
| Passed | 2441 |
| Failed | 0 |
| Skipped | 4 |
| Repetitions | 3 |
| Warmup | 1 |

The README block under `sqlite-parity-report:begin` repeats these counts and dates the run 2026-09-23. The badge above it that says 2374/2445 is an older drawing. The runner skips cases that require `fts5`, `rtree`, or `dbstat` when the target probe fails. That is the source of the 4 skips.

## Postgres

Corpus size, from `subrepos/redline-testing/corpus/beyond_sqlite/generated_manifest.json`: **265** cases.

Open failures, from `metadata/beyond_sqlite/postgres-regression.json`: **25** cases.

Policy reason, copied from that file: plpgsql functions, procedures, and notify-from-plpgsql run. 25 failures remain.

Oracle settings the gate requires: `160015|C|C|UTC`.

The generated README paragraph between `POSTGRES_PARITY_START` and `POSTGRES_PARITY_END` is the report merged in `f8a0b7c2d` and still describes an earlier measurement. Use the table below.

| Category | Case | Name | What the case is about |
| --- | ---: | --- | --- |
| BEYOND_LISTEN_NOTIFY | 20438 | `LISTEN_INVALID_CHANNEL_NAME_ALL` | LISTEN ALL is a syntax error (ALL is not a valid identifier here, unlike UNLISTEN *) |
| BEYOND_MVCC_LOCKING | 20404 | `SELECT_FOR_KEY_SHARE` | SELECT ... FOR KEY SHARE acquires a weaker key-share lock |
| BEYOND_MVCC_LOCKING | 20405 | `SELECT_FOR_NO_KEY_UPDATE` | SELECT ... FOR NO KEY UPDATE allows concurrent FOR KEY SHARE |
| BEYOND_MVCC_LOCKING | 20412 | `LOCK_TABLE_ACCESS_EXCLUSIVE` | LOCK TABLE ... IN ACCESS EXCLUSIVE MODE succeeds in a single tx with no contender |
| BEYOND_PORTABILITY_SYNTAX | 20103 | `MERGE_NOT_MATCHED_BY_SOURCE_PG17` | MERGE ... WHEN NOT MATCHED BY SOURCE is PG17+; PG16 should error |
| BEYOND_PORTABILITY_SYNTAX | 20111 | `DISTINCT_ON_REQUIRES_ORDER_BY` | DISTINCT ON expressions must be the leftmost ORDER BY terms; mismatch errors |
| BEYOND_REPLICATION_CDC | 20418 | `LOGICAL_SLOT_REJECTS_WAL_LEVEL_REPLICA` | pg_create_logical_replication_slot rejects slot creation when wal_level < logical |
| BEYOND_REPLICATION_CDC | 20423 | `PG_EXPORT_SNAPSHOT_SHAPE` | pg_export_snapshot() returns a nonempty string identifier (value is nondeterministic) |
| BEYOND_REPLICATION_CDC | 20425 | `CREATE_PUBLICATION_FOR_ALL_TABLES` | CREATE PUBLICATION FOR ALL TABLES round-trips with idempotent drop |
| BEYOND_REPLICATION_CDC | 20429 | `LOGICAL_SLOT_PEEK_REQUIRES_SLOT` | pg_logical_slot_peek_changes errors when slot does not exist |
| BEYOND_RICH_TYPES | 20021 | `ENUM_REJECTS_BAD_VALUE` | ENUM cast rejects values outside the declared set |
| BEYOND_RICH_TYPES | 20023 | `DOMAIN_REJECTS_NEG` | DOMAIN check rejects bad value at cast |
| BEYOND_STORED_PROCEDURES | 20308 | `PLPGSQL_RAISE_EXCEPTION` | RAISE EXCEPTION aborts execution with nonzero exit |
| BEYOND_VECTOR_ADVANCED_INDEXES | 20340 | `VECTOR_EXTENSION_PROBE_UNAVAILABLE` | CREATE EXTENSION vector probe — fails gracefully when pgvector is not installed |
| BEYOND_VECTOR_ADVANCED_INDEXES | 20343 | `INDEX_GIST_RANGE` | CREATE INDEX USING gist on int4range supports overlap query |
| BEYOND_VECTOR_ADVANCED_INDEXES | 20348 | `TSVECTOR_BASIC_MATCH` | to_tsvector + to_tsquery + @@ predicate match |
| BEYOND_VECTOR_ADVANCED_INDEXES | 20349 | `TSVECTOR_SETWEIGHT` | setweight applies A/B/C/D rank weights to lexemes |
| BEYOND_VECTOR_ADVANCED_INDEXES | 20350 | `TSVECTOR_GIN_INDEX` | GIN index over to_tsvector(...) expression supports @@ search |
| BEYOND_VECTOR_ADVANCED_INDEXES | 20351 | `TSVECTOR_TS_RANK` | ts_rank returns a deterministic non-negative score for a match |
| BEYOND_VECTOR_ADVANCED_INDEXES | 20352 | `PG_TRGM_EXTENSION_AND_SIMILARITY` | CREATE EXTENSION pg_trgm IF NOT EXISTS + similarity() returns expected score |
| BEYOND_VECTOR_ADVANCED_INDEXES | 20353 | `PG_TRGM_GIN_INDEX` | GIN index with gin_trgm_ops supports % similarity search |
| BEYOND_VECTOR_ADVANCED_INDEXES | 20354 | `PG_TRGM_GIST_INDEX` | GiST index with gist_trgm_ops supports % similarity search |
| BEYOND_VECTOR_ADVANCED_INDEXES | 20355 | `PG_TRGM_WORD_SIMILARITY` | word_similarity + <-> trigram distance are deterministic |
| BEYOND_VECTOR_ADVANCED_INDEXES | 20358 | `BTREE_GIN_SCALAR` | btree_gin extension enables GIN over plain scalar columns |
| BEYOND_VECTOR_ADVANCED_INDEXES | 20359 | `BTREE_GIST_SCALAR` | btree_gist extension enables GiST over plain scalar columns |

A case leaves this table when a raw beyond-SQLite JSONL record shows `status` passed and the policy file drops the id. A shared non-zero exit stays a failure until the case declares `expected_target_stderr_contains` and the target produces that text.

## Where the other chapters point

| Question | File |
| --- | --- |
| Feature-by-feature SQLite map | `docs/sqlite-parity.md` |
| Why some Postgres cases are deferred in the skip list | `docs/beyond-postgres-skips.md` |
| RQL document shape | `docs/rql.md` |
| Crate boundaries | `docs/boundaries.md` |
| Installer details that drift | `docs/install_redlinedb.md` is older than [Start here](02-start-here.md) |

