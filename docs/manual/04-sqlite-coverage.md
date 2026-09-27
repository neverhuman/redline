# SQLite coverage

RedlineDB's SQLite story is a measured shell and library comparison, plus a file format of its own.

The official lane builds SQLite 3.53.1 with `scripts/sqlite/build-reference.sh` and compares it to `redlinedb` from the same checkout. The runner is `subrepos/redline-testing`. Engine unit tests are useful, and they are not that lane. A number in this chapter comes from the committed report, not from a unit test.

## The committed result

| | |
| --- | --- |
| Corpus | `sqlite_parity`, 2445 cases |
| Passed | 2445 |
| Failed | 0 |
| Skipped | 0 |
| Repetitions | 3, plus 1 warmup |
| Report date in the README block | 2026-09-24 |
| RedlineDB build named in that block | `redlinedb v4.1.0 (SQLite 3.45.1 compatibility)` |
| Oracle | SQLite 3.53.1, 2026-05-05 |
| Runner | `redline-testing` 1.0.1 |
| Source file | `benchmark-results/sqlite-parity/latest/summary.json` |

The README badge and the block under `sqlite-parity-report:begin` are generated from this summary and the official evidence behind it. Both name the corpus and the oracle, not SQLite as a whole: the badge reads `SQLite SQL/CLI corpus`, and it is green only for a run with official evidence, no failures and no skips. The same block records latency as per-case RedlineDB/SQLite ratios of CLI process wall time (lower is better): median **5.72x**, p95 **56.85x**, worst **299.21x**, and **43** of 2445 cases faster than SQLite. Those timings come from a 128-worker conformance run, not a tuned benchmark. A later bench moves those figures by regenerating the block. This book does not invent a new one.

## Virtual tables the corpus asks for

`CREATE VIRTUAL TABLE ... USING fts5`, `rtree`, or `dbstat` creates an ordinary table and the official cases compare its rows.

- `fts5` stores the named columns. `docs MATCH 'hello'` is true when a column contains that text, and `highlight` wraps the term the `MATCH` just used.
- `rtree` stores the coordinate columns. The corpus filters them with ordinary comparisons.
- `dbstat` is a one-row table, so `count(*) > 0` is `1`.

An unknown module still fails with `CREATE VIRTUAL TABLE is not supported without module migration support`. `pragma_module_list` also prints `fts3`, `fts4`, `fts5vocab`, `dbpage`, and the pragma helpers. Those extra names do not create a table. This is not the SQLite C virtual-table API.

The report lists cases 00093-00096 as declared deviations, with 10405 (`pragma_module_list`) and 12023 (`PRAGMA compile_options`, a fixed copy of the reference build's options). Each passes on output produced without the SQLite feature behind it. The list is `subrepos/redline-testing/metadata/sqlite_parity/declared-deviations.json`.

## What the passing surface feels like

On the statements the corpus compares, you can expect the ordinary SQLite shape:

- `CREATE TABLE`, `INSERT`, `UPDATE`, `DELETE`, `SELECT`
- joins, compound `UNION` / `INTERSECT` / `EXCEPT`, and common table expressions, including recursive ones
- window functions (`row_number`, `rank`, `lag`, `lead`, and the usual frames)
- views and row triggers, with the gaps called out in `docs/sqlite-parity.md` (for example, `INSTEAD OF` triggers on views)
- `RETURNING`, savepoints, and generated columns
- JSON and math functions that the 3.53.1 oracle was built with

`docs/sqlite-parity.md` is the feature ledger. Its status column is `pass`, `partial`, `fail`, `not-started`, or `rejects-by-design`. Read it when you are about to depend on one feature. The official 2445 on this branch is the corpus result. The ledger is the map of which SQL features that result is made of.

## Two SQLite behaviors this build shares with the reference shell

The reference shell is built without `SQLITE_SOUNDEX`. `soundex()` is a missing function there, and it is a missing function here. Turning it on in RedlineDB alone would make the parity diff worse, so this build leaves it off.

The reference parser was generated without the grammar for `UPDATE` or `DELETE` ... `ORDER BY` ... `LIMIT`. Those statements are a syntax error on the oracle (`near "ORDER"`). RedlineDB does not accept them either. A flag that only Redline understood would not be SQLite compatibility.

The reference shell does enable the session extension. `.session` with no arguments prints help and exits 0. That is a shell command, not a SQL function.

## Files

The path you open is a directory. `data.redline` holds pages. `wal/` holds the log. The constants are `RDPG` and `RDWL` stored little-endian, so a hex dump does not begin with those four letters. Tools that inspect SQLite format 3 headers will not describe `data.redline` correctly. Take a logical dump (SQL text) when you need to move rows into SQLite, and load a dump when you need to move rows out. The directory stays with the engine that wrote it.

## How to re-check

From a build of this repository:

```bash
just redline-testing-official
```

That is the long lane. `just fast` is the short local check. It does not replace the official corpus. The report generator expects `benchmark-results/sqlite-parity/latest/provenance.json` before it rewrites the README block.

Next, if you are moving statements: [SQL you will write](06-sql-you-will-write.md). If the scripts came from Postgres: [Postgres coverage](05-postgres-coverage.md).
