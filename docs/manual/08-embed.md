# Embed it

Four ways lead into the engine. Pick the one that matches the process you already have.

## Rust

The crate is `redlinedb`. The types you should name in application code are the ones re-exported from `crates/redlinedb/src/lib.rs`: `Database`, `Connection`, `Transaction`, `Statement`, `Row`, `Value`, `Params`, `OpenOptions`, `Durability`, `Pool`, `Error`, and the RQL types (`RqlProgram`, `RqlStatement`, and the rest of that list).

```rust doctest
use redlinedb::{Database, Durability, OpenOptions};

fn open() -> redlinedb::Result<Database> {
    let options = OpenOptions {
        durability: Durability::Strict,
        ..OpenOptions::default()
    };
    Database::open_with_options(std::env::temp_dir().join("demo.redline"), options)
}
// prints: 1
```

`Database::create` is the shorter call. It sets `create: true` and keeps every other default, including `Strict`. Use `open_with_options` when you need to change one of those defaults. `Database::open` also uses `OpenOptions::default()`, and that default has `create: true`, so a missing directory is created. The path is a directory in every case. A regular file at that path returns `database path exists as a regular file`.

`query_row` maps one column into a Rust type that implements `FromValue`. `execute` runs a statement that does not return the rows you want to walk. `prepare` returns a `Statement` you can `step`.

With the `tokio` feature the crate also exports `AsyncDatabase` and `AsyncConnection`. The sync types remain the ones the rest of this book shows.

Do not add a dependency on `redlinedb-kernel` or `redlinedb-sql` in an application. Those crates move. The facade is the contract.

## C

`crates/ffi` presents a SQLite-shaped C ABI. Symbols keep `sqlite3_*` names so a program written to that API can be aimed at this library. The headers in the tree are `contracts/c-abi/redlinedb.h` and `contracts/c-abi/sqlite3.h`. `scripts/install-from-source.sh` installs both. The release package script includes both headers.

The ABI covers the calls the FFI tests exercise. It does not grow a SQLite feature that the SQL engine refuses. FTS5, R-tree and `dbstat` have corpus-shaped stand-ins rather than full
SQLite modules; a passing corpus case does not establish their complete API. Link the library. Do not `dlopen` an extension from SQL. There is no `load_extension` entry point to receive it.

The release tarball includes the library and the headers next to the `redlinedb` binary. A program that only needs the shell can ignore the library.

## RQL

RQL is the embedded API for a caller that should not produce SQL text. Construct an `RqlStatement` in Rust, or send the JSON document the shell reads. The phase-1 document can create and drop tables and indexes, insert, update, delete, and select, with filters, joins, grouping, ordering, limits, scalar expressions, and simple subqueries.

```bash doctest
redlinedb --rql :memory: < docs/manual/examples/items.rql.json
# prints: Ada
```

The lowering step builds the same kind of plan SQL would. Results come back through the shell's normal output modes. `--rql` is exclusive: the process is either reading that JSON document or it is reading SQL, not both in one invocation.

RQL is young. It is the right tool when the caller is a program. It is the wrong tool when you are trying to prove SQLite compatibility. The SQLite lane sends SQL.

## The TCP server

`redlinedb-server` listens on TCP and frames messages with the magic bytes `RLDB` and protocol version 1. Each message is a JSON object with a `cmd` tag. The commands in `crates/server/src/main.rs` are `hello`, `prepare`, `bind`, `step`, `reset`, `finalize`, `exec`, `begin`, `commit`, `rollback`, `interrupt`, and `close`.

This is a small protocol for a process that wants the engine in another address space. It is not Postgres wire protocol. It has no SQLSTATE. It is not a substitute for `psql`. If your client is a Postgres driver, run Postgres.

The installer from [Start here](02-start-here.md) installs `redlinedb-server` beside `redlinedb`. The process requires both flags. `--database` names a RedlineDB directory; `Database::open` creates it when missing and rejects a regular file. `--listen` is the address you choose. There is no default port.

```bash
redlinedb-server --database /tmp/demo.redline --listen 127.0.0.1:7422
```

`7422` is only an example. Pick a free port on localhost until something in front of this process authenticates clients. The protocol itself does not.

## The web console

The optional `redline-web` package is a console in front of a `redlinedb` binary:

```bash
redline-web --target-bin /path/to/redlinedb
```

It is a way to look at a database you already run. It is not a second engine. Queries it sends go through the binary you named.

## Pools

`PoolBuilder` configures a `Pool` of connections on one `Database`. Use it when many tasks in one process need a connection and you do not want each task to call `connect` with its own policy. Session state is per connection. Dropping a pooled connection rolls back an open transaction before the connection returns to the idle list, so the next checkout does not inherit that transaction. A listen channel set on the connection is not cleared by that drop. Clear it yourself if the next borrower should not see it.

Next: [Files and day-to-day operation](09-operate.md) for backups and for the rule about one writing process.
