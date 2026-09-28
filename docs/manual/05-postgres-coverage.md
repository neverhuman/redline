# Postgres coverage

Postgres compatibility in this release is a SQL-shell comparison with PostgreSQL 16.15. The client is the `redlinedb` shell. The oracle is `psql` against that server. The wire protocol Postgres clients speak is a different project. `psql` cannot connect to `redlinedb-server`.

Set the dialect when you want this behavior:

```bash
REDLINEDB_RESULT_DIALECT=postgres redlinedb /tmp/app.redline
```

Leave it unset for SQLite work. The official SQLite lane does not set it.

## The committed result

| | |
| --- | --- |
| Corpus | `beyond_sqlite`, 265 cases |
| Oracle | PostgreSQL 16.15, settings `160015\|C\|C\|UTC` |
| Image the gate accepts | `sha256:efdf07c2f9d4df592783dcc8ea5f6db02efbf5f6452b527225ff5e58364570e9` |
| Open failures in the policy on this commit | 0 |
| Policy file | `metadata/beyond_sqlite/postgres-regression.json` |
| Policy reason | text search and the nine declared rejections agree; no open failures remain |

The policy's `failed_cases` array is the list the gate allows. On this branch that array is empty. A local gate recorded 265 passed, 0 failed, 0 skipped, and the regression check passed. The README block between `POSTGRES_PARITY_START` and `POSTGRES_PARITY_END` is that generated result.

Nine of the 265 passes are agreed rejections. Both engines exit 3, stdout matches, and the case declares `expected_target_stderr_contains`. The target produced that text, and setup completed. Those statements still fail. The [ledger](appendix-coverage.md) names them. The other 256 passes return rows.

## What already behaves like Postgres

These are in the engine on this commit, and the corpus cases that cover them are not in the open list.

**Results.** Booleans render as `t` and `f`. `NULL` renders as the null marker the shell was given.

**What a match means.** A passing case is *normalized SQL-shell transcript agreement*: `psql` and the `redlinedb` shell print the same text once the case's normalizers run. Both shells print the ASCII unit separator (0x1F) between cells and `NULL` wrapped in ASCII record separators (0x1E) for a SQL NULL, so `('a|b','c')` and `('a','b|c')` differ, and NULL, the text `'NULL'`, and `''` differ. It is still a text comparison. A value that contains a newline prints like two rows, and the normalizers (for example `t`/`f` to `1`/`0`) act on cells without their SQL types. It is not typed-result or application parity.

**Names.** A schema-qualified table stays one name. `auth.users` and `public.users` are different tables. Unqualified lookup uses the search path when that encoded name exists. Sequence names still store the bare name.

**Types you can create.**

- `CREATE TYPE name AS ENUM (...)` stores the labels in declaration order. `'meh'::mood < 'sad'::mood` follows that order, which is not alphabetical.
- `CREATE DOMAIN ... CHECK (VALUE > n)` returns the base value when the check passes and rejects the value when it fails.
- `int4range(lo, hi)` is half-open. `int4range(1, 10)` contains 5 and does not contain 10. Overlap (`&&`) uses the same bounds.
- `point(x, y)` and the `<->` operator. `point(0,0) <-> point(3,4)` is `5`.
- `CREATE EXTENSION citext` compares and orders without case, and the value keeps the spelling you wrote. `CREATE EXTENSION pg_trgm`, `btree_gin`, and `btree_gist` are accepted. `CREATE EXTENSION vector` fails with `extension "vector" is not available`.
- `to_tsvector`, `to_tsquery`, `setweight`, `ts_rank`, `@@`, `similarity`, `word_similarity`, `%`, and text `<->` match the corpus rows. `USING gin` and `USING gist` are stored as ordinary indexes. The cases compare rows, not plans.

**SQL functions.** `LANGUAGE SQL` functions run. `LATERAL` is accepted on the forms the corpus covers. `DEFAULT nextval(...)` together with `ALTER SEQUENCE ... OWNED BY` inserts sequence values.

**Session state.** `LISTEN` and `UNLISTEN` record channels on this connection. A listen that has not committed is gone after rollback. `pg_listening_channels()` reads that set. `NOTIFY` is accepted and does not deliver a payload to any session. The open listen/notify cases are `LISTEN ALL`, which is a syntax error, and `pg_notify` inside a function or a trigger.

**Table flags.** `ALTER TABLE ... INHERIT` makes a later read of the parent return the parent rows and the child rows. `NO INHERIT` drops the child from that read. `SET UNLOGGED` then `SET LOGGED` reports `relpersistence` as `u` then `p`. `SET STATISTICS 250` reads back `250`. `SET STORAGE EXTERNAL` reads back `e`. `array_to_string(reloptions, ',')` reads back `autovacuum_enabled=true` after `SET (autovacuum_enabled = true)`. `OWNER TO CURRENT_USER` reports `tableowner` as `redlinedb`. `SET WITHOUT CLUSTER` leaves the clustered-index count at `0`.

**Catalogs the shell can see.** Empty shims exist for several `pg_*` views the corpus only counts (`pg_locks`, publication and subscription views, replication slots). `pg_class`, `pg_namespace`, and `pg_constraint` can be rewritten from the session snapshot so a name probe returns rows. Materialized views have a session catalog. Read `docs/sqlite-parity.md` and the SQL crate when you need the exact column list. A shim that answers `count(*)` is not the full Postgres catalog.

## Agreed errors

These nine statements fail on both engines. The corpus records the RedlineDB text, so the gate counts them as passes. They are not queries you can build on.

| Case | What you get |
| ---: | --- |
| 20021 | `invalid input value for enum color: "purple"` |
| 20023 | `value for domain positive_int2 violates check constraint "positive_int2_check"` |
| 20103 | `syntax error at or near "BY"` for `MERGE ... WHEN NOT MATCHED BY SOURCE` |
| 20111 | `SELECT DISTINCT ON expressions must match initial ORDER BY expressions` |
| 20308 | `ERROR: boom` from `RAISE EXCEPTION` |
| 20340 | `extension "vector" is not available` |
| 20418 | `logical decoding requires wal_level >= logical` |
| 20429 | the same logical-decoding error from `pg_logical_slot_peek_changes` |
| 20438 | `syntax error at or near "ALL"` for `LISTEN ALL` |

The corpus plpgsql bodies run: assignment, `IF`, loops, `RETURN NEXT`, `RETURN QUERY`, `CALL`, `STRICT`, `VARIADIC int[]`, and `PERFORM pg_notify` from a function or an `AFTER INSERT` trigger. `RAISE EXCEPTION` aborts with the message above. A plpgsql program outside those shapes is still unsupported.

`USING gin` and `USING gist` in the passing index cases become ordinary indexes. Point distance works. A passing overlap query is not a different access plan.

## A practical migration order

1. Run the schema and the queries with the dialect variable set, against a throwaway file.
2. Anything that returns `UnsupportedSql` is a stop. Look the statement up in the ledger before you rewrite it.
3. Keep cross-session `NOTIFY`, logical replication, and `vector` on Postgres. Corpus `plpgsql` runs; `RAISE EXCEPTION` aborts.
4. Move the rest — tables, SQL functions, enums, ranges, citext, and ordinary `SELECT` — when the shell output matches the `psql` output you care about.

The gate command used in CI is the `beyond_sqlite` suite of `redline-testing`, with `REDLINE_TESTING_POSTGRES_URL` pointing at the pinned server and `REDLINE_TESTING_POSTGRES_IMAGE` set to the digest above. `ops/ci/parity.sh` fills the image digest in when it finds that local server.

Next: [SQL you will write](06-sql-you-will-write.md) for the statements to type instead, then [Transactions and durability](07-transactions.md).
