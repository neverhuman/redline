# Files and day-to-day operation

## Where the bytes are

`Database::create` takes a directory path. The engine rejects a regular file at that path. Inside the directory, the heap image is `data.redline` and the write-ahead log is `wal/`. Copying the directory copies both. Copying `data.redline` alone leaves the log behind, and a crash recovery on that lone file is not a supported backup. Use the backup commands below, or shut the process down and copy the whole directory.

`:memory:` and an empty filename are process-local. They are the right target for a test and the wrong target for a notebook you want tomorrow.

Page and log constants are `RDPG` and `RDWL` stored little-endian, as [Why RedlineDB](01-why-redlinedb.md) describes. Tools that assume a SQLite format 3 header will misread `data.redline`. Dump to SQL when a foreign tool has to see the rows.

## Backup and stats from the shell

Day-to-day commands:

```bash
redlinedb /tmp/demo.redline "SELECT count(*) FROM kv"
redlinedb stats /tmp/demo.redline --json
redlinedb backup /tmp/demo.redline /tmp/demo.bak --physical
```

The first form is the shell: database path, then SQL. `stats DB --json` prints schema epoch, resident heap pages, and write-ahead log positions. `backup SRC DST --physical` copies at the storage layer. `--logical` is the other mode. Prefer either backup to `cp` on a live file. The maintenance commands beside these are `restore`, `archive-check`, `replication-slot`, `stream-wal`, and `stream-logical`.

The Rust API mirrors this with `BackupOptions`, `PhysicalBackupOptions`, and the stats structs re-exported from `redlinedb` (`DatabaseStats`, `CommitStats`, `WalBenchStats`, and the others in that `pub use`). A program that must backup on a schedule should call that API inside the process that already holds the `Database`, so the copy sees a quiescent engine rather than a file mid-write.

## Checkpoints and vacuum

The stats structs include checkpoint and vacuum counters. Run them when the write-ahead log has grown and you want it folded back into the database, or when you have deleted a large fraction of a table and want the file to shrink. They are maintenance, not part of a request an agent should issue on every turn. An agent that vacuums after every insert will dominate the runtime with maintenance.

## One writer process, many connections

Open the file from one `Database` and hand out connections. Two processes opening the same path for write depend on the process owner lock (`OpenOptions.process_owner_lock`, default on). That lock exists so a second process does not treat the file as its own. If you need two operating-system processes, run `redlinedb-server` in one of them and let the other speak `RLDB`. Two uncoordinated `Database::create` calls on one path are not the multi-agent design. A pool inside one process is.

## Install layout

A release install puts `redlinedb` and `redlinedb-server` on `PATH` under the prefix you chose, default `~/.local`. Headers and the native library are in that prefix as well. The optional packages are:

| Package | What it is |
| --- | --- |
| `redlinedb-v4.1.0-<platform>.tar.gz` | Engine, CLI, server, FFI |
| `redline-testing-v4.1.0-<platform>.tar.gz` | The conformance runner |
| `redline-web-v4.1.0-<platform>.tar.gz` | The web console |

Each package carries dependency notices, an SBOM, and the parent commit it was built from. Verify the checksum before you run the binary on a machine an agent can reach. `REDLINEDB_SHA256` makes the installer require the digest you pass.

## Upgrading

A directory written by this commit opens in an engine built from this commit. A future version may bump the catalog format. Read the release notes before you point a new binary at an old directory, and take a physical backup first. This manual describes commit `8ae3a8b79`. `--version` still prints `4.1.0` for the older git tag of that number, so the version line does not tell those builds apart. Re-read the coverage chapters against the `summary.json` and `postgres-regression.json` in the checkout you are actually running before you repeat a count.

## Logs and quiet mode

The library writes a one-line notice to stderr the first time `REDLINEDB_DEFAULT_DURABILITY` selects a mode other than the default. `REDLINEDB_QUIET_DURABILITY=1` turns that notice off. Leave it visible in development so a benchmark environment does not quietly become the production environment.
