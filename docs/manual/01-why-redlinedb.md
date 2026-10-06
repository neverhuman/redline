# Why RedlineDB

RedlineDB keeps SQL inside the process that needs it. There is no separate database server to provision, no role to create, and no port to open before the first `CREATE TABLE`. A Rust program calls `Database::create`, receives a connection, and runs statements. A shell user points `redlinedb` at a directory path. The same engine answers both. The engine creates that directory if it is missing. Inside it, the heap image is `data.redline` and the log is `wal/`. A regular file at that path is rejected.

The storage core is MVCC. A transaction you begin takes a snapshot. A statement outside a transaction uses read committed. A commit publishes a new snapshot after the durability step you selected. Writers do not need readers to finish before they proceed, and the next open after a crash recovers from the write-ahead log. Pages are Redline pages. The page constant is the ASCII codes for `RDPG` (`0x52445047`), written little-endian, so a hex dump of a page starts with `47 50 44 52`. The log constant is `RDWL` (`0x5244574C`), written the same way, so the log starts with `4C 57 44 52`. SQLite format 3 is an import and export concern. Pointing `redlinedb` at a SQLite database file fails, because that path is a regular file.

## What you gain

**One engine, two familiar SQL shapes.** The default shell speaks a SQLite-shaped dialect: dot-commands, `INTEGER PRIMARY KEY`, and the official parity corpus. Set `REDLINEDB_RESULT_DIALECT=postgres` when the statement and the expected rendering come from Postgres. Predicate booleans can render as `t` and `f`; literals and stored boolean values can render as `1` and `0`. The Postgres chapter lists what that mode has been shown to do, and what it still refuses.

**A typed program for agents, beside SQL.** [RQL](../rql.md) is a JSON document of create, insert, update, delete, and select operations. The engine lowers that document into the same executor plans SQL uses. The text is not passed through the SQL parser. An agent that can emit JSON can talk to the database without building a SQL string. Chapter [For agents](03-for-agents.md) is the short version.

**Durability you can name.** The default open mode is `Strict`: `COMMIT` returns, and the commit becomes visible to other snapshots, only after its log record is fsynced. The [durability contract](durability.md) says which failures that survives and which statement a test receipt backs; power loss is not claimed. `Normal` and `UnsafeDev` exist for benchmarks and short-lived processes. The environment variable is `REDLINEDB_DEFAULT_DURABILITY`. `strict` and `full` select Strict. `normal` selects Normal. `unsafe_dev`, `unsafe-dev`, and `off` select UnsafeDev. Any other string panics at the first default `OpenOptions`. The panic text names `strict`, `normal`, and `unsafe_dev`. The aliases above are accepted even though that text does not list them.

**A C ABI with SQLite symbol names, and a Rust facade that stays small.** `crates/ffi` exports `sqlite3_*` names for programs that already know that ABI. It is an experimental subset, not a replacement `libsqlite3`; [`docs/api-stability.md`](../api-stability.md) lists each symbol's status. The supported Rust API is what `crates/redlinedb/src/lib.rs` re-exports. Reaching into `redlinedb-kernel` or `redlinedb-sql` from an application skips the boundary the rest of the stack is tested against.

**A measured compatibility story.** Every claim of "works like SQLite" or "works like Postgres" in this book points at a corpus, a date, and a file. The project would rather show a skip than call it a pass.

## When to choose it

Choose RedlineDB when the program is Rust, the data fits an embedded file, and the SQL you need sits inside the covered SQLite surface or the covered Postgres shell surface. Choose it when several agents in one process must read a stable snapshot while another agent commits. Choose it when you want the query itself to be data (RQL) rather than a string the model assembled.

Choose SQLite when you must open an existing SQLite format 3 file in place, when you need FTS5, R-tree, or `dbstat`, or when your own queries measure slower here and that matters more than the embedded Rust API. Latency against the SQLite shell is measured on a quiet host in the README's Versions over time table; the conformance report is not a benchmark. Read [SQLite coverage](04-sqlite-coverage.md) before you promise a speedup.

Choose Postgres when you need the wire protocol, full `plpgsql`, logical replication, row-level lock modes, or an extension such as `vector`. The shell lane is a SQL comparison. It is not a server you can point `psql` at. The TCP server in this tree speaks a small framed protocol whose magic is `RLDB`. Chapter [Embed it](08-embed.md) describes that protocol.

## What this book will not do

It will not restate the engine's internal task list. It will not copy a score into a sentence that the repository cannot regenerate. Where the README and a policy file disagree, the chapter names both and says which one to trust.

Next: [Start here](02-start-here.md).
