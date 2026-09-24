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
| Open failures in the policy on this commit | 20 |
| Policy file | `metadata/beyond_sqlite/postgres-regression.json` |
| Policy reason | row locks, LOCK TABLE, snapshot export, and CREATE PUBLICATION run |

The policy's `failed_cases` array is the list the gate allows. A run passes the regression check when every failure is in that list and every case in that list still fails. When a case starts passing, it has to leave the list. The list is not a trophy. It is the set of known misses.

The README block between `POSTGRES_PARITY_START` and `POSTGRES_PARITY_END` still reports **127 passed, 138 failed** from commit `090dbc9b`. That block is generated, and it has not been regenerated on this commit. Until CI writes a new block, quote the policy file: **20** named failures out of **265** cases.

A matching non-zero exit is not a pass. The runner counts agreement on an error only when the case declares the text the target must produce. Several of the 20 are errors we intend to keep, once that text is declared. They are listed in the [ledger](appendix-coverage.md).

## What already behaves like Postgres

These are in the engine on this commit, and the corpus cases that cover them are not in the open list.

**Results.** Booleans render as `t` and `f`. `NULL` renders as the null marker the shell was given (the parity preamble uses `.nullvalue NULL`).

**Names.** A schema-qualified table stays one name. `auth.users` and `public.users` are different tables. Unqualified lookup uses the search path when that encoded name exists. Sequence names still store the bare name.

**Types you can create.**

- `CREATE TYPE name AS ENUM (...)` stores the labels in declaration order. `'meh'::mood < 'sad'::mood` follows that order, which is not alphabetical.
- `CREATE DOMAIN ... CHECK (VALUE > n)` returns the base value when the check passes and rejects the value when it fails.
- `int4range(lo, hi)` is half-open. `int4range(1, 10)` contains 5 and does not contain 10. Overlap (`&&`) uses the same bounds.
- `point(x, y)` and the `<->` operator. `point(0,0) <-> point(3,4)` is `5`.
- `CREATE EXTENSION citext` compares and orders without case, and the value keeps the spelling you wrote. `CREATE EXTENSION vector` fails. The vector extension is not bundled.

**SQL functions.** `LANGUAGE SQL` functions run. `LATERAL` is accepted on the forms the corpus covers. `DEFAULT nextval(...)` together with `ALTER SEQUENCE ... OWNED BY` inserts sequence values.

**Session state.** `LISTEN` and `UNLISTEN` record channels on this connection. A listen that has not committed is gone after rollback. `pg_listening_channels()` reads that set. `NOTIFY` is accepted and does not deliver a payload to any session. The open listen/notify cases are `LISTEN ALL`, which is a syntax error, and `pg_notify` inside a function or a trigger.

**Table flags.** `ALTER TABLE ... INHERIT` makes a later read of the parent return the parent rows and the child rows. `NO INHERIT` drops the child from that read. `SET UNLOGGED` then `SET LOGGED` reports `relpersistence` as `u` then `p`. `SET STATISTICS 250` reads back `250`. `SET STORAGE EXTERNAL` reads back `e`. `array_to_string(reloptions, ',')` reads back `autovacuum_enabled=true` after `SET (autovacuum_enabled = true)`. `OWNER TO CURRENT_USER` reports `tableowner` as `redlinedb`. `SET WITHOUT CLUSTER` leaves the clustered-index count at `0`.

**Catalogs the shell can see.** Empty shims exist for several `pg_*` views the corpus only counts (`pg_locks`, publication and subscription views, replication slots). `pg_class`, `pg_namespace`, and `pg_constraint` can be rewritten from the session snapshot so a name probe returns rows. Materialized views have a session catalog. Read `docs/sqlite-parity.md` and the SQL crate when you need the exact column list. A shim that answers `count(*)` is not the full Postgres catalog.

## What is still open

The 20 names are in the [ledger](appendix-coverage.md). They fall into a few jobs:

| Open cases | Job |
| ---: | --- |
| 1 | `RAISE EXCEPTION` inside `DO`. The other corpus plpgsql bodies run |
| 12 | Text search, trigram, GiST, GIN, and the `vector` extension |
| 2 | Logical decoding while `wal_level` is below `logical`. Publications and `pg_export_snapshot` already run |
| 1 | `LISTEN ALL` |
| 2 | An enum label that is not in the type, and a domain value that fails its check. These are errors on Postgres too. They stay failures until the corpus declares the message. |
| 2 | `MERGE ... WHEN NOT MATCHED BY SOURCE` (a Postgres 17 clause; this oracle is 16.15) and the `DISTINCT ON` form whose expressions are not the leftmost `ORDER BY` terms. Other `DISTINCT ON` cases are not in the open list |

The corpus plpgsql bodies run: assignment, `IF`, loops, `RETURN NEXT`, `RETURN QUERY`, `CALL`, `STRICT`, `VARIADIC int[]`, and `PERFORM pg_notify` from a function or an `AFTER INSERT` trigger. `RAISE EXCEPTION` still fails. A plpgsql program outside those shapes is still unsupported.

GiST and GIN in that table are index methods the corpus asks for. Point distance works without a GiST index. Do not read a passing distance expression as proof that `USING gist` changes the plan.

## A practical migration order

1. Run the schema and the queries with the dialect variable set, against a throwaway file.
2. Anything that returns `UnsupportedSql` is a stop. Look the statement up in the ledger before you rewrite it.
3. Keep `plpgsql`, `NOTIFY` between sessions, logical replication, and `vector` on Postgres.
4. Move the rest — tables, SQL functions, enums, ranges, citext, and ordinary `SELECT` — when the shell output matches the `psql` output you care about.

The gate command used in CI is the `beyond_sqlite` suite of `redline-testing`, with `REDLINE_TESTING_POSTGRES_URL` pointing at the pinned server and `REDLINE_TESTING_POSTGRES_IMAGE` set to the digest above. `ops/ci/parity.sh` fills the image digest in when it finds that local server.

Next: [SQL you will write](06-sql-you-will-write.md) for the statements to type instead, then [Transactions and durability](07-transactions.md).
