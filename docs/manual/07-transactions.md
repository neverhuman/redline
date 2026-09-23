# Transactions and durability

A commit in RedlineDB is a snapshot that other readers can see, plus a write-ahead record that decides whether that snapshot survives a crash. Those two steps are ordered on purpose.

## What a transaction sees

Application code starts a transaction with SQL `BEGIN` or with `Connection::begin`. The argument is a `BeginMode`, not an isolation level.

| `BeginMode` | What it does |
| --- | --- |
| `Deferred` | Starts a snapshot transaction. `conn.transaction(...)` uses this mode. |
| `Immediate` | Starts a snapshot transaction and reserves the begin lock. |
| `Exclusive` | Starts a snapshot transaction and reserves the begin lock. |

A statement outside `BEGIN` uses the kernel's read-committed path. All three begin modes call the kernel with `Isolation::Snapshot`. The rows you read inside that transaction stay the rows that were published when the snapshot was taken. A commit that publishes later stays invisible until you begin again. That is the mode an agent wants when it reads several tables and must not observe a torn write from a peer.

The kernel also knows `Isolation::ReadCommitted` and `Isolation::Serializable`. Read committed is used on some internal statement paths. Serializable is refused: `Engine::begin(Isolation::Serializable)` returns `UnsupportedIsolation`. There is no method on `Connection` that asks for serializable isolation and succeeds. Stay on `BeginMode`.

`COMMIT`, `ROLLBACK`, and savepoints are on the SQL surface the parity ledger marks as covered. Nested savepoints are part of that coverage. A `LISTEN` issued inside a transaction is undone if that transaction rolls back. An uncommitted listen disappears, which is the Postgres rule.

```rust
conn.begin(redlinedb::BeginMode::Deferred)?;
conn.execute("INSERT INTO note (body) VALUES (?)", redlinedb::params![body])?;
conn.commit()?;
```

## When the commit is real

`Durability` on `OpenOptions` has three values.

| Mode | Meaning |
| --- | --- |
| `Strict` | The default. The flush finishes, and only then is the commit published to other snapshots. A crash during the flush does not leave the new row visible. |
| `Normal` | Schema changes are written and the durability barrier is lighter. The parity harness sets this when it is measuring SQL rather than fsync. |
| `UnsafeDev` | Skips shutdown flush work that the other modes still do. For development processes you can afford to throw away. |

If `REDLINEDB_DEFAULT_DURABILITY` is unset, `OpenOptions::default()` selects `Strict`. `strict` and `full` select Strict. `normal` selects Normal. `unsafe_dev`, `unsafe-dev`, and `off` select UnsafeDev. Any other string panics the first time default options are built. The panic text names `strict`, `normal`, and `unsafe_dev`.

`REDLINEDB_QUIET_DURABILITY=1` suppresses the one-line notice the library prints the first time a non-default mode is selected through the environment.

The order on the strict path is the product rule: durable, then visible. Code that published the commit and flushed afterwards could show a row that a crash then lost. That path is gone. If you are writing a tool that reports "committed" to a person, leave the default in place and report success after `COMMIT` returns.

## Locks and busy waits

`OpenOptions.busy_timeout` defaults to five seconds. A lock wait that exceeds it returns to the caller. The Postgres row-lock forms still open on this commit are `FOR KEY SHARE`, `FOR NO KEY UPDATE`, and `LOCK TABLE`. `FOR UPDATE` and `FOR SHARE` are not in that open list. A timeout is how long this process will wait when the engine's own lock manager is busy. It is not those SQL clauses.

`InterruptHandle`, exported next to `Connection`, cancels a statement. Use it for an agent deadline. Use `busy_timeout` for a lock wait. They are different clocks.

## Several connections, one directory

`Database::connect` opens another connection on the same engine. `Pool` checks connections out. Each connection has its own session state (listen channels, Postgres types created in that session). The directory, `data.redline`, and `wal/` are shared. A connection inside `BEGIN` has its own snapshot. A connection that has not begun a transaction reads through the read-committed path.

A reader on one connection does not have to finish before a writer on another connection commits. The reader keeps the snapshot it started with. The writer publishes a new one, subject to the durability mode.

## Crash

Recovery replays the write-ahead log. On `Strict`, a commit that did not finish its flush is not a published snapshot, so recovery does not have to invent a story about a row that other transactions already observed. After recovery, open the file again with the same `Database` API. There is no separate recovery command for the common case.

Take a backup before you experiment with `UnsafeDev` on a file you care about. Chapter [Files and day-to-day operation](09-operate.md) shows the backup commands.
