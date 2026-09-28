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
| `Strict` | The default. `COMMIT` returns after the commit record is fsynced to the write-ahead log. Other snapshots see the commit only after that fsync. |
| `Normal` | `COMMIT` returns after the commit record is written to the operating system with `write(2)`, before any fsync. It survives the process dying, not an OS crash or power loss. The schema file is written without fsync too. The parity harness sets this when it is measuring SQL rather than fsync. |
| `UnsafeDev` | `COMMIT` returns once the commit record is queued, before the operating system has the bytes. The process dying can lose commits that returned. Closing also skips the final log flush. For development processes you can afford to throw away. |

If `REDLINEDB_DEFAULT_DURABILITY` is unset, `OpenOptions::default()` selects `Strict`. `strict` and `full` select Strict. `normal` selects Normal. `unsafe_dev`, `unsafe-dev`, and `off` select UnsafeDev. Any other string panics the first time default options are built. The panic text names `strict`, `normal`, and `unsafe_dev`.

`REDLINEDB_QUIET_DURABILITY=1` suppresses the one-line notice the library prints the first time a non-default mode is selected through the environment.

The order is the product rule in every mode: past the mode's barrier, then visible. On `Strict` that means durable, then visible. Code that published the commit and flushed afterwards could show a row that a crash then lost. That path is gone.

`COMMIT` returns only once a transaction that begins afterwards sees the commit, on any connection, including the one that committed. Commits become visible in the order they reached the log. When an earlier commit has passed its barrier but is not visible yet, a later `COMMIT` waits for it. Snapshots that were already open do not change. One exception: a schema change such as `CREATE TABLE` reaches new statements when its transaction publishes, which can be a moment before that transaction's rows are visible.

If you are writing a tool that reports "committed" to a person, leave the default in place and report success after `COMMIT` returns.

Which failures each mode survives, and which of those statements a test receipt backs, is in the [durability contract](durability.md). A power cut is not one of them: no release has a power-loss receipt.

## When `COMMIT` fails

A `COMMIT` error means one of two things.

- The commit record never reached the log queue, for example because an earlier log write had already failed. The transaction is rolled back. It does not come back.
- The commit record was queued, and then writing or fsyncing the log failed before the commit passed its barrier. The outcome is unknown. SQL returns `CommitMaybeCommitted` ("commit outcome uncertain"), which the `redlinedb` crate reports as `ErrorCode::IoErr` and the C API as `RLDB_IOERR`. The kernel's error is `CommitOutcomeUnknown`, which names the transaction, the log position its record ends at, and the log failure.

After an unknown outcome, this process does not show the transaction. The log writer stops at its first failure, so every later write fails with an error that says `wal writer failed` and names the step (`write`, `flush` or `rotate`). Close the database and open it again. If the commit record reached the file whole, recovery replays it and the transaction is there. Otherwise it is not. A failed fsync does not prove the bytes are absent.

Do not retry a change that must happen once just because `COMMIT` failed. Reopen, then check whether it happened, for example by looking up a key the transaction wrote.

### Not yet certified

The guarantees on this page are the design rule, and the kernel's crash-recovery tests (failpoint injection and the recovery matrix under `crates/kernel/tests/` and `crates/bench/`) check it by killing a process and recovering. This release has not been certified against power loss or against a storage device that drops or reorders writes it reported as flushed, and recovery defects found by the launch audit are tracked in `GROK_GAPS.md`. Keep backups (chapter [Files and day-to-day operation](09-operate.md)) of any database you cannot recreate.

## Locks and busy waits

`OpenOptions.busy_timeout` defaults to five seconds. A lock wait that exceeds it returns to the caller. `FOR KEY SHARE`, `FOR NO KEY UPDATE`, and `LOCK TABLE` return the same rows as the statement without a wait. A timeout is how long this process will wait when the engine's own lock manager is busy.

`InterruptHandle`, exported next to `Connection`, cancels a statement. Use it for an agent deadline. Use `busy_timeout` for a lock wait. They are different clocks.

## Several connections, one directory

`Database::connect` opens another connection on the same engine. `Pool` checks connections out. Each connection has its own session state (listen channels, Postgres types created in that session). The directory, `data.redline`, and `wal/` are shared. A connection inside `BEGIN` has its own snapshot. A connection that has not begun a transaction reads through the read-committed path.

A reader on one connection does not have to finish before a writer on another connection commits. The reader keeps the snapshot it started with. The writer publishes a new one, subject to the durability mode.

## Crash

Recovery replays the write-ahead log. Every commit whose record is whole in the log comes back. That includes a commit whose `COMMIT` had not returned yet, or returned an unknown outcome. On `Strict`, a commit is visible, and `COMMIT` returns, only after its record is fsynced, so recovery finds every commit that another transaction saw or that `COMMIT` reported, as long as the storage keeps what fsync acknowledged. `Normal` and `UnsafeDev` give that up for the cases in the table above. The [durability contract](durability.md) lists the failure models, the receipt behind the process-kill statement, and the known gaps; [Not yet certified](#not-yet-certified) summarizes them. Recovery runs when you open the file again with the same `Database` API. There is no separate recovery command for the common case.

Take a backup before you experiment with `UnsafeDev` on a file you care about. Chapter [Files and day-to-day operation](09-operate.md) shows the backup commands.
