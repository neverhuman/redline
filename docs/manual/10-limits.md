# Limits

This page is the one to hand to someone who is about to promise RedlineDB as a drop-in. Every line here is true of commit `8ae3a8b79`.

## The file

The database path is a directory containing `data.redline` and `wal/`. SQLite format 3 tools do not open that image. Postgres does not open it. Moving a deployment means a SQL dump, a restore, or the backup API. It does not mean handing the directory to a `sqlite3` binary.

## SQLite

The official corpus recorded 2441 passes, 0 failures, and 4 skips out of 2445 on the run dated 2026-09-18. The skips are the cases that need `fts5`, `rtree`, or `dbstat`. `CREATE VIRTUAL TABLE` for an unmigrated module fails with a clear error. `pragma_module_list` still prints those module names. The print is not an implementation.

`soundex()` is absent, matching a reference build that was not compiled with it. `UPDATE` and `DELETE` with `ORDER BY` ... `LIMIT` are syntax errors, matching a reference parser that was not generated with that grammar.

The median case in that latency report was slower than SQLite. 361 cases were faster. Quote both if you quote either.

The C ABI uses `sqlite3_*` names for the calls it implements. A symbol existing is not a promise that every SQLite C flag and every SQLite authorizer hook exists. Read `crates/ffi` and `docs/boundaries.md` for the surface you are linking.

## Postgres

The shell comparison has 265 cases. The regression policy names 25 that still fail. The README's generated Postgres paragraph is older (127 passed, 138 failed) and should not be quoted as the current policy.

Still open, in plain language:

- `RAISE EXCEPTION`, and plpgsql outside the corpus shapes (assignment, `IF`, loops, `RETURN NEXT`, `RETURN QUERY`, `CALL`, `STRICT`, one `VARIADIC int[]`)
- text search, trigram similarity, and GiST/GIN index methods
- the `vector` extension
- row locks that are still open: `FOR KEY SHARE`, `FOR NO KEY UPDATE`, and `LOCK TABLE`
- publications, exported snapshots, and logical decoding
- `pg_notify` inside a function or a trigger (plain `NOTIFY` is accepted and delivers nothing)
- `MERGE` syntax that belongs to Postgres 17
- two error strings (a bad enum label, a domain check) that Postgres also rejects, and that the gate will not count as agreed until the expected text is declared

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

The 25 open Postgres cases are a work list, not a law. When one starts passing, the policy file loses that id and this chapter goes stale in that one row. The SQLite summary file moves when a new official run is committed. If you are reading this on a commit other than `8ae3a8b791d4edab88cf8513ad0d99ef709a1202`, regenerate your confidence from those two files before you repeat a count.
