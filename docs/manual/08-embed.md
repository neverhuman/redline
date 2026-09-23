# Embed it

Four ways lead into the engine. Pick the one that matches the process you already have.

## Rust

The crate is `redlinedb`. The types you should name in application code are the ones re-exported from `crates/redlinedb/src/lib.rs`: `Database`, `Connection`, `Transaction`, `Statement`, `Row`, `Value`, `Params`, `OpenOptions`, `Durability`, `Pool`, `Error`, and the RQL types (`RqlProgram`, `RqlStatement`, and the rest of that list).

```rust
use redlinedb::{Database, Durability, OpenOptions};

fn open() -> redlinedb::Result<Database> {
    let options = OpenOptions {
        durability: Durability::Strict,
        ..OpenOptions::default()
    };
    Database::open_with_options("/tmp/demo.redline", options)
}
```

`Database::create` is the shorter call. It turns `create: true` on and keeps every other default, including `Strict`. Use `open_with_options` when you need to change one of those defaults. `Database::open` opens an existing file with default options.

`query_row` maps one column into a Rust type that implements `FromValue`. `execute` runs a statement that does not return the rows you want to walk. `prepare` returns a `Statement` you can `step`.

With the `tokio` feature the crate also exports `AsyncDatabase` and `AsyncConnection`. The sync types remain the ones the rest of this book shows.

Do not add a dependency on `redlinedb-kernel` or `redlinedb-sql` in an application. Those crates move. The facade is the contract.

## C

`crates/ffi` presents a SQLite-shaped C ABI. Symbols keep `sqlite3_*` names so a program written to that API can be aimed at this library. The header shipped with the product is `crates/ffi/include/redlinedb.h`.

The ABI covers the calls the FFI tests exercise. It does not grow a SQLite feature that the SQL engine refuses. A C program that calls into FTS5, R-tree, or `dbstat` meets the same absence the shell meets. Link the library. Do not `dlopen` an extension from SQL. There is no `load_extension` entry point to receive it.

The release tarball includes the library and the headers next to the `redlinedb` binary. A program that only needs the shell can ignore the library.

## RQL

RQL is the embedded API for a caller that should not produce SQL text. Construct an `RqlStatement` in Rust, or send the JSON document the shell reads. The phase-1 document can create and drop tables and indexes, insert, update, delete, and select, with filters, joins, grouping, ordering, limits, scalar expressions, and simple subqueries.

```bash
redlinedb --rql /tmp/items.redline < docs/manual/examples/items.rql.json
```

The lowering step builds the same kind of plan SQL would. Results come back through the shell's normal output modes. `--rql` is exclusive: the process is either reading that JSON document or it is reading SQL, not both in one invocation.

RQL is young. It is the right tool when the caller is a program. It is the wrong tool when you are trying to prove SQLite compatibility. The SQLite lane sends SQL.

## The TCP server

`redlinedb-server` listens on TCP and frames messages with the magic bytes `RLDB` and protocol version 1. Each message is a JSON object with a `cmd` tag. The commands in `crates/server/src/main.rs` are `hello`, `prepare`, `bind`, `step`, `reset`, `finalize`, `exec`, `begin`, `commit`, `rollback`, `interrupt`, and `close`.

This is a small protocol for a process that wants the engine in another address space. It is not Postgres wire protocol. It has no SQLSTATE. It is not a substitute for `psql`. If your client is a Postgres driver, run Postgres.

The installer from [Start here](02-start-here.md) installs `redlinedb-server` beside `redlinedb`. The process requires both flags. `--database` is an existing file (`Database::open`). `--listen` is the address you choose. There is no default port.

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

`PoolBuilder` configures a `Pool` of connections on one `Database`. Use it when many tasks in one process need a connection and you do not want each task to call `connect` with its own policy. Session state is per connection. A pooled connection returned dirty — mid-transaction, or with a listen channel you did not mean to share — will surprise the next checkout. Commit or roll back before you return it.

Next: [Files and day-to-day operation](09-operate.md) for backups and for the rule about one writing process.
