# paper/ — historical preprint

> **Historical and unreconciled. Not evidence for any current release.**

This directory holds a preprint written in May 2026 (its sections were last
changed on 2026-05-24) and the data and scripts used to build it. It is kept for
the record. Do not cite its numbers as a description of RedlineDB today, and do
not cite it in place of the software: use [CITATION.cff](../CITATION.cff).

## Why it is not current evidence

- **Methodology counts do not reconcile.** `sections/methodology.tex` describes
  8 workloads × 2 durability modes × 8 thread levels × 5 measured repetitions
  × 2 engines = 1,280 measured runs, with one warmup per cell. That implies 256
  warmups and 1,536 runs in total, but the text says 448 warmups and 1,728
  runs. `data/cert_totals.csv` records 1,280 measured runs but 1,440 measured
  runs in the JSONL. The certification config it cites
  (`crates/bench/bench/certification.toml`) lists 9 workloads, not 8, and
  names some of them differently.
- **Failure counts disagree with its data.** The paper reports "0 hard
  failures" (`sections/methodology.tex:41`, `sections/evaluation.tex:196`)
  without defining the term, while `data/perf_aggregates.csv` line 31 records
  `failures_total` = 3631 for RedlineDB on `hot-row-update`, Strict, 64
  threads (SQLite's row on line 172 records 143). Which of those failures, if
  any, the paper counted as hard is not recorded.
- **Different comparison.** The SQLite baseline was the SQLite bundled by the
  `rusqlite` crate on one host, in a multi-threaded throughput harness. Current
  release evidence compares the `redlinedb` shell with the pinned sqlite3 3.53.1
  shell, case by case.
- **Claims since narrowed.** The paper says only the FFI shim contains `unsafe`
  blocks; the tree also has `unsafe` outside the FFI crate, for example in
  SIMD, morsel-filter, executor and JSONB code, recorded in
  `.jankurai/unsafe-ledger.toml`. It says RedlineDB is API-compatible with
  SQLite; the C ABI is an experimental subset and the SQL surface is measured
  on a corpus, with any failing cases listed.
- **Old source.** The evaluation cites a phase-10 certification at commit
  `7c10410`, long before v5.0.0.
- **Anonymous draft.** The author block is an anonymous-submission placeholder.

## Where current numbers live

- The README's generated blocks: the SQLite SQL/CLI corpus report, the
  PostgreSQL 16.15 SQL-shell corpus block, and the **Versions over time** table.
- [`benchmark-results/`](../benchmark-results/): the official evidence, reports,
  and the release bench bundle under
  [`benchmark-results/sqlite-parity/releases/`](../benchmark-results/sqlite-parity/releases/).
- [`docs/performance-history.md`](../docs/performance-history.md) for older
  published figures and their caveats.

Regenerating the paper against current evidence is deferred.

`figs/architecture.tex` was corrected for v5.0.0 (the C client box, the C ABI
box and the `unsafe` note), and the README's `assets/architecture.png` is
rendered from it. `main.pdf` was not rebuilt and still shows the old figure.
