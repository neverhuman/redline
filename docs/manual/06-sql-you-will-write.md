# SQL you will write

Most statements look like the engine you already know. The surprises are small, and they are where bugs hide.

## Pick a dialect for the whole process

The dialect is an environment variable, `REDLINEDB_RESULT_DIALECT`.

| Value | Use it for |
| --- | --- |
| unset | SQLite scripts, the official SQLite corpus, and Rust examples in this book |
| `postgres` | Scripts you would run in `psql`, and the beyond-SQLite corpus |

One process should stick to one value. A connection opened under the Postgres dialect stores Postgres session state (enums, domains, citext, listen channels, schemas). Mixing the two in one directory is how a later `SELECT` sees a name or a boolean you did not expect.

## Types that differ by dialect

**Booleans.** In the Postgres dialect they render as `t` and `f`. SQLite-shaped callers often see integers. If you parse output, branch on the dialect instead of accepting both.

**Enums.** Order is the order of the labels in `CREATE TYPE`, from first to last. Sorting an enum column alphabetically in the client will disagree with `ORDER BY` in the engine.

**Ranges.** `int4range(1, 10)` contains every integer from 1 up to and excluding 10. A predicate copied from a closed-interval library will be off by one at the upper bound.

**Citext.** Equality ignores case. The stored text keeps its original letters. If you strip case yourself before insert, you have thrown away the spelling citext was keeping.

**JSON.** SQLite JSON functions that the parity corpus covers are available in the SQLite dialect. Postgres JSON rendering is only as compatible as the cases that have left the open list. When a JSON test fails, compare the text. Key order and whitespace are the usual cause.

## Names

The catalog stores every table name folded to ASCII lowercase. `"Users"` and `users` are one table. Quote style does not preserve case in the name the engine looks up.

In the Postgres dialect, `schema.table` is one identifier. Creating `auth.users` does not also create a table named `users` that another schema can see. Drop the qualified name you created.

## Statements people paste in from elsewhere

These are the ones that look portable and are not, on this commit:

- `CREATE VIRTUAL TABLE ... USING fts5` (and `rtree`, and `dbstat`)
- plpgsql outside the shapes in [Postgres coverage](05-postgres-coverage.md), including `RAISE EXCEPTION`
- `CREATE EXTENSION vector`
- `UPDATE ... ORDER BY ... LIMIT` and the `DELETE` form of the same
- `soundex()`
- `MERGE` with `WHEN NOT MATCHED BY SOURCE`
- `LISTEN ALL`

`LANGUAGE sql` functions, ordinary `SELECT` and DML, `WITH`, windows, and the types in the previous section are the statements to build on.

## Parameters

In Rust, pass values through `Params`. The statement text stays constant.

```rust
conn.execute("INSERT INTO note (body) VALUES (?)", params![body])?;
```

The exact placeholder style follows the SQL parser's SQLite-shaped parameter rules (`?`, and the numbered forms the corpus accepts). If a snippet from Postgres uses `$1`, try it under the Postgres dialect and keep the value in a parameter either way. Building the value into the string with `format!` is how a note body becomes a second statement.

RQL avoids the question. The value is a JSON field. See [For agents](03-for-agents.md) and [`examples/items.rql.json`](examples/items.rql.json).

## Errors worth reading once

`UnsupportedSql` means the parser or the planner recognized the statement and will not run it. Changing whitespace will not help.

The kernel's message for serializable isolation is `unsupported isolation level`. The Rust facade surfaces that as a generic error, not as its own `ErrorCode`. SQL `BEGIN` and `Connection::begin` do not ask for serializable isolation. They start a snapshot. See [Transactions and durability](07-transactions.md).

A domain check failure and an unknown enum label are data errors. They are supposed to fail. The open list still contains those two cases because the corpus has not yet declared the exact error text the gate should require. The rejection itself is the Postgres behavior.

## Nulls and headers in the shell

Scripts that diff output should set the shell the way the corpus does:

```text
.mode list
.headers off
.separator |
.nullvalue NULL
```

Without that preamble, a correct row can look like a mismatch. The engine did not change. The printer did.

Next: [Transactions and durability](07-transactions.md), then [Embed it](08-embed.md).
