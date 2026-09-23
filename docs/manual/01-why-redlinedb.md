# Why RedlineDB

RedlineDB keeps SQL inside the process that needs it. There is no separate database server to provision, no role to create, and no port to open before the first `CREATE TABLE`. A Rust program calls `Database::create`, receives a connection, and runs statements. A shell user points `redlinedb` at a file path. The same engine answers both.

The storage core is MVCC. Readers take a snapshot. A commit publishes a new snapshot after the durability step you selected. Writers do not need readers to finish before they proceed, and a crash recovers from the write-ahead log. Pages are Redline pages. The magic at the start of a page is `RDPG` (`0x52445047`). Write-ahead records use `RDWL` (`0x5244574C`). Those bytes are how you recognize a Redline file. SQLite format 3 is an import and export concern. Opening a SQLite database file directly in `redlinedb` is not the supported path.

## What you gain

**One engine, two familiar SQL shapes.** The default shell speaks a SQLite-shaped dialect: dot-commands, `INTEGER PRIMARY KEY`, and the official parity corpus. Set `REDLINEDB_RESULT_DIALECT=postgres` when the statement and the expected rendering come from Postgres. Booleans then render as `t` and `f`. The Postgres chapter lists what that mode has been shown to do, and what it still refuses.

**A typed program for agents, beside SQL.** [RQL](../rql.md) is a JSON document of create, insert, update, delete, and select operations. The engine lowers that document into the same executor plans SQL uses. The text is not passed through the SQL parser. An agent that can emit JSON can talk to the database without building a SQL string. Chapter [For agents](03-for-agents.md) is the short version.

**Durability you can name.** The default open mode is `Strict`: the commit becomes visible to other snapshots only after the flush that makes it durable. `Normal` and `UnsafeDev` exist for benchmarks and short-lived processes. The environment variable is `REDLINEDB_DEFAULT_DURABILITY` with values `strict`, `normal`, and `unsafe_dev`. An unknown value panics at the first default `OpenOptions`, so a typo does not silently weaken the database.

**A C ABI with SQLite symbol names, and a Rust facade that stays small.** `crates/ffi` exports `sqlite3_*` names for programs that already know that ABI. The supported Rust API is what `crates/redlinedb/src/lib.rs` re-exports. Reaching into `redlinedb-kernel` or `redlinedb-sql` from an application skips the boundary the rest of the stack is tested against.

**A measured compatibility story.** Every claim of "works like SQLite" or "works like Postgres" in this book points at a corpus, a date, and a file. The project would rather show a skip than call it a pass.

## When to choose it

Choose RedlineDB when the program is Rust, the data fits an embedded file, and the SQL you need sits inside the covered SQLite surface or the covered Postgres shell surface. Choose it when several agents in one process must read a stable snapshot while another agent commits. Choose it when you want the query itself to be data (RQL) rather than a string the model assembled.

Choose SQLite when you must open an existing SQLite format 3 file in place, when you need FTS5, R-tree, or `dbstat`, or when the official latency report shows your query in the slow tail and that tail matters more than the embedded Rust API. The committed SQLite report records a median gap where RedlineDB is slower, and 361 cases where it is faster. Read [SQLite coverage](04-sqlite-coverage.md) before you promise a speedup.

Choose Postgres when you need the wire protocol, `plpgsql`, logical replication, row-level lock modes, or an extension such as `vector`. The shell lane is a SQL comparison. It is not a server you can point `psql` at. The TCP server in this tree speaks a small framed protocol whose magic is `RLDB`. Chapter [Embed it](08-embed.md) describes that protocol.

## What this book will not do

It will not restate the engine's internal task list. It will not copy a score into a sentence that the repository cannot regenerate. Where the README and a policy file disagree, the chapter names both and says which one to trust.

Next: [Start here](02-start-here.md).
