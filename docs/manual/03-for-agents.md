# For agents

An agent fails in a database in a small number of ways. It concatenates a value into SQL. It reports success before the write is durable. It loads code the user did not ask for. It assumes two engines with the same keyword accept the same statement. RedlineDB has specific answers to those failures. They are part of the product, and they are narrower than a slogan.

## Send a document, or send SQL

SQL remains the compatibility language. Use it when you are replaying a SQLite script or a Postgres script that sits inside the covered surface.

When the agent is the author of the query, prefer [RQL](../rql.md). RQL version 0.1 is a typed relational program. It is off unless you call it. The Rust methods are `Connection::prepare_rql`, `Connection::execute_rql`, and `Database::prepare_rql`. The shell form is:

```bash
redlinedb --rql /tmp/items.redline < docs/manual/examples/items.rql.json
```

`--rql` reads one JSON document from stdin. It cannot be combined with `--cmd` or a SQL argument. The document's statements are `create_table`, `insert`, `select`, and the other phase-1 forms listed in `docs/rql.md`. The engine lowers them to prepared plans. It does not render SQL text and parse it again. A value inside the JSON is a value. It is not a slice of the statement.

That is the gain other embedded SQL engines do not ship: a second, typed front end that lands in the same executor, for callers that should never build SQL by hand.

If you do send SQL, keep values in parameters. The Rust `Params` trait is the supported way. `redlinedb::sql_input_complete` returns true when a buffer holds one finished statement. A REPL uses it to decide the user is done typing. It does not execute the statement.

## A snapshot is a stable read

`BEGIN`, and `Connection::begin` with any `BeginMode`, starts a snapshot. `Deferred`, `Immediate`, and `Exclusive` differ only in whether the begin lock is reserved. A commit that had not been published when the snapshot was taken stays invisible to that transaction. Serializable isolation is refused inside the kernel, and `Connection` has no method that requests it. The calls are in [Transactions and durability](07-transactions.md).

`Strict` durability, the default, flushes the write-ahead record before that commit is published. An agent that must not tell a user "saved" before a crash would still find the row should leave the default alone. Set `REDLINEDB_DEFAULT_DURABILITY=normal` only for a benchmark or a throwaway directory. `unsafe_dev` (also `unsafe-dev` and `off`) skips work that `normal` still does. It is for development, and the name is the warning. The accepted names are listed in [Transactions and durability](07-transactions.md).

## The process boundary stays in Rust

There is no `load_extension` entry point in the engine. A statement that asks for one fails. Native code enters the process through the host program's link line. An agent cannot turn that on by sending SQL.

`CREATE VIRTUAL TABLE ... USING fts5`, `rtree`, or `dbstat` creates an ordinary table. `MATCH` scans that row's text, and `highlight` wraps the term. An unknown module still fails. `pragma_module_list` also prints names that do not create a table (`fts3`, `fts4`, `dbpage`). Trust a `CREATE VIRTUAL TABLE` that succeeds.

## Two dialects, one flag

Agents often emit Postgres even when the file is local. `REDLINEDB_RESULT_DIALECT=postgres` (read once when the shell starts; `DbOptions::dialect` in Rust) switches result rendering and the Postgres-oriented rewrites the shell corpus uses. Booleans come back as `t` and `f`. Schema-qualified names stay distinct, so `auth.users` and `public.users` do not collapse into one table. `CREATE TYPE ... AS ENUM` compares labels in declaration order. `CREATE DOMAIN ... CHECK (VALUE > n)` accepts a value that passes the check and rejects one that fails. `int4range` is half-open. `point` distance uses `<->`. A value cast with `::citext` compares without case and preserves the spelling you stored; a column declared `citext` does not, so treat `citext` as partial.

That flag does not turn the process into Postgres. The corpus subset of `plpgsql` runs. `RAISE EXCEPTION` aborts with `ERROR: boom`. `CREATE EXTENSION vector` fails with `extension "vector" is not available`. Text search, trigram, and the GIN/GiST forms in the corpus return rows; those indexes are ordinary indexes. `LISTEN` records a channel on this connection and restores the set on rollback. It is not a cross-process notification bus, and `NOTIFY`, advisory locks, transaction ids and WAL LSNs are stand-ins. There is no PostgreSQL wire protocol, TLS, roles or SQLSTATE. The Postgres chapter and its capability matrix are the list.

SQLite tests and SQLite scripts should leave the variable unset. The official SQLite lane does.

## Stop a runaway statement

`InterruptHandle` is part of the public connection API. Hold it in the supervisor task and interrupt the statement when the agent's deadline expires. A busy timeout on `OpenOptions` defaults to five seconds and covers lock waits. Interrupts and busy timeouts answer different waits. Set both when an agent must bound its own runtime.

## Read the ledger before you retry

When a statement fails, read the error. `UnsupportedSql` and `UnsupportedIsolation` mean the engine recognized the shape and refused it. Retrying the same text will fail again. The [coverage ledger](appendix-coverage.md) lists the Postgres cases that are still expected to fail on this commit. An agent that treats every error as a prompt to rewrite the SQL will loop on those cases. An agent that treats the ledger as input will switch strategies: RQL or SQLite-shaped SQL for the job it can finish, and a real Postgres server for the job it cannot.

## A connection pool inside the process

`Pool` and `PoolBuilder` live in the `redlinedb` crate. With the `tokio` feature, `AsyncDatabase` and `AsyncConnection` are exported as well. Several agent tasks can check out connections without opening a new file each time. They still share one MVCC engine. They do not get a network hop unless you deliberately run `redlinedb-server`.

## What to put in the prompt you give an agent

A useful system prompt for this database is short:

- Prefer parameters, or prefer RQL, when the agent authors the query.
- Leave the dialect at SQLite (`REDLINEDB_RESULT_DIALECT` unset for the shell, `DbOptions::dialect` at its default in Rust) for SQLite scripts. Choose the Postgres subset only for Postgres scripts.
- Do not claim FTS5, R-tree, `dbstat`, full `plpgsql`, `vector`, or the Postgres wire protocol.
- Treat `Strict` as the default. Say so when you report that a commit succeeded.
- On `UnsupportedSql`, stop and consult the ledger. Do not loop.

The calls named above are written out in [Embed it](08-embed.md). The commit rule is in [Transactions and durability](07-transactions.md).
