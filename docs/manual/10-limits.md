# Limits

This page is the one to hand to someone who is about to promise RedlineDB as a drop-in. Every line here is true of commit `8ae3a8b79`.

## The file

The database path is a directory containing `data.redline` and `wal/`. SQLite format 3 tools do not open that image. Postgres does not open it. Moving a deployment means a SQL dump, a restore, or the backup API. It does not mean handing the directory to a `sqlite3` binary.

## SQLite

The committed official summary is 2445 passes, 0 failures, and 0 skips out of 2445, with 3 repetitions and 1 warmup. `fts5`, `rtree`, and `dbstat` are ordinary tables for the corpus statements. An unknown module still fails. `pragma_module_list` also prints names that do not create a table. The README badge is generated from that same summary.

`soundex()` is absent, matching a reference build that was not compiled with it. `UPDATE` and `DELETE` with `ORDER BY` ... `LIMIT` are syntax errors, matching a reference parser that was not generated with that grammar.

The median case in that latency report was slower than SQLite. 361 cases were faster. Quote both if you quote either.

The C ABI uses `sqlite3_*` names for the calls it implements. A symbol existing is not a promise that every SQLite C flag and every SQLite authorizer hook exists. Read `crates/ffi` and `docs/boundaries.md` for the surface you are linking.

## Postgres

The shell comparison has 265 cases. The regression policy's failure list is empty. Its declared-unsupported list names the 11 cases RedlineDB refuses with `unsupported capability:` while PostgreSQL succeeds: they call `pg_current_wal_lsn`, `pg_export_snapshot`, `pg_notify` or `NOTIFY`. A local gate recorded 254 agreeing, 11 declared unsupported and 0 mismatches. 12 of the agreeing cases are expected rejections, listed in [Postgres coverage](05-postgres-coverage.md): a bad enum label, a failing domain check, `MERGE ... WHEN NOT MATCHED BY SOURCE`, a `DISTINCT ON` order mismatch, two explicit values in `GENERATED ALWAYS AS IDENTITY` columns, a NULL after `ALTER COLUMN ... SET NOT NULL`, `RAISE EXCEPTION`, `CREATE EXTENSION vector`, logical decoding while `wal_level` is `replica` (two cases), and `LISTEN ALL`. Agreement is normalized SQL-shell transcript agreement, not the PostgreSQL wire protocol, TLS, roles, or SQLSTATE; the [capability matrix](../beyond-postgres-skips.md#capability-matrix) lists those as unsupported.

Still outside the corpus, in plain language:

- plpgsql outside the shapes that run (assignment, `IF`, loops, `RETURN NEXT`, `RETURN QUERY`, `CALL`, `STRICT`, one `VARIADIC int[]`)
- the `vector` extension
- logical decoding and `NOTIFY`: nothing delivers a notification, so `NOTIFY` and `pg_notify` fail with `unsupported capability:`
- WAL LSNs and snapshot export: `pg_current_wal_lsn` and `pg_export_snapshot` fail with `unsupported capability:`
- advisory locks between processes, and the shared and transaction-level advisory locks; session-level advisory locks work between the connections of one process

`LISTEN` on this connection works, and it rolls back with the transaction. It does not fan out to other processes.

There is no Postgres wire server. `libpq`, `psql`, and ORMs that speak that protocol need Postgres.

`Isolation::Serializable` is refused.

## Agents

RQL covers the phase-1 relational document. It is not the full SQL language in JSON form. A statement RQL cannot express has to be SQL, and then it has to be SQL this engine runs.

An agent loop that retries `UnsupportedSql` will not make progress. The [ledger](appendix-coverage.md) is the list of Postgres statements where retrying is the wrong move.

## Speed

Do not plan a migration on the hope that every query gets faster. Measure the queries you run. The performance recipes in `just/lanes.just` are `perf-full`, `perf-pgo`, `perf-bolt`, and `phase9-certify`. The official parity runner accepts `--repetitions 3`. Keep the raw output. A change that wins a microbenchmark and loses a parity pass is not a win the project keeps.

## Process model

One `Database` value owns the file for writing. Other agents in the same process take connections or pool checkouts. Other operating-system processes talk to `redlinedb-server` and its `RLDB` protocol, or they wait their turn. The server protocol is version 1, framed JSON, magic `RLDB`. It does not authenticate clients. `redlinedb-server` requires `--database` and `--listen`. Pass a localhost address until you have put your own authentication in front of it.

## What a later commit can change

The policy file and a fresh `sqlite_parity` run are the counts to repeat. The committed latency summary moves when a new official report is committed. If you are reading this on a later commit, regenerate your confidence from those files before you repeat a count.
