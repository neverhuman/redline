# The RedlineDB manual

This is the book for people who will open a database, send it SQL, and embed it in a program. It was written against commit `45fc7dbe608ddafc949ec6ecf5264262ad7a4d1a` (version 4.1.0, 23 September 2026). When a number in a later chapter disagrees with a generated badge, the chapter says which file is the measurement.

RedlineDB is an embedded SQL engine written in Rust. A program links it, or a person runs the `redlinedb` shell. The path you pass is a directory. Inside it, pages live in `data.redline` and the write-ahead log lives in `wal/`. Page and log constants are the ASCII codes for `RDPG` and `RDWL`, stored little-endian. The shell accepts a large SQLite-shaped vocabulary. A separate mode renders a slice of Postgres 16.15 results. Both compatibility programs are measured, and both are unfinished.

## Ten minutes

Read these three, in order, if that is all the time you have.

1. [Why RedlineDB](01-why-redlinedb.md) — what you gain, and when another engine is the better tool.
2. [Start here](02-start-here.md) — install, create a file, run one query from the shell and from Rust.
3. [For agents](03-for-agents.md) — the surfaces that exist so a program can drive the database without guessing.

## The rest of the book

| Chapter | You will know |
| --- | --- |
| [SQLite coverage](04-sqlite-coverage.md) | What the official 2445-case lane measured, including the 4 skips. |
| [Postgres coverage](05-postgres-coverage.md) | What the 265-case shell lane measures, and the 48 cases still open on this commit. |
| [SQL you will write](06-sql-you-will-write.md) | Dialects, types, and a few results that surprise people coming from the other engine. |
| [Transactions and durability](07-transactions.md) | Snapshots, the isolation modes the kernel accepts, and when a commit is durable. |
| [Embed it](08-embed.md) | Rust, the C ABI, RQL, and the small TCP server. |
| [Files and day-to-day operation](09-operate.md) | Paths, backup, stats, and the installer. |
| [Limits](10-limits.md) | The boundaries worth remembering before you promise them to someone else. |
| [Coverage ledger](appendix-coverage.md) | The open Postgres cases, copied from the regression policy. |

## How to treat the numbers

Two lanes produce the compatibility numbers in this book.

The SQLite lane compares `redlinedb` with SQLite 3.53.1 on the official `sqlite_parity` corpus. The figure quoted here is the committed report in [`benchmark-results/sqlite-parity/latest/summary.json`](../../benchmark-results/sqlite-parity/latest/summary.json): **2441 passed, 0 failed, 4 skipped**, out of 2445, dated 2026-09-18 in the README report block. A badge higher in the README still says 2374/2445. That badge is older than the report block. Use the report block.

The Postgres lane compares the shell with PostgreSQL 16.15 on 265 cases. The committed regression policy [`metadata/beyond_sqlite/postgres-regression.json`](../../metadata/beyond_sqlite/postgres-regression.json) names **48** cases that are still failing on this commit. The generated README block between `POSTGRES_PARITY_START` and `POSTGRES_PARITY_END` still shows an older run (127 passed, 138 failed) from commit `090dbc9b`. Use the policy file until CI rewrites that block.

A skip is a case the runner did not compare. A listed failure is a case that ran and did not match. Neither one is a pass.

## Examples you can run

- [`examples/first.sql`](examples/first.sql) — a table, a row, a read.
- [`examples/items.rql.json`](examples/items.rql.json) — the same idea as a typed RQL document, so an agent does not have to concatenate SQL.

Build the shell from this checkout with `./scripts/build-from-source.sh` when you want the binary that matches the commit above. The install chapter has the release-package path.
