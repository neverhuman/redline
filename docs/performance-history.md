# Performance history (historical)

This page keeps performance figures that earlier READMEs published, so that
links and citations still resolve. **None of them describes v5.0.0**, and most
of them cannot be reproduced. Each section says what it measured, how, and why
it should not be compared with current numbers.

For current figures, use:

- the **Versions over time** table in the [README](../README.md#versions-over-time), which re-runs every listed release on today's corpus against SQLite (the note under the table says how it was measured), and
- the release bench bundle it is rendered from, [`benchmark-results/sqlite-parity/releases/v5.0.0/`](../benchmark-results/sqlite-parity/releases/v5.0.0/), which holds the raw records, host and build contracts, and the summary.

Every ratio on this page is RedlineDB latency divided by SQLite latency for one
per-process CLI case, so **lower is better**, and a value above 1.0 means
RedlineDB was slower.

## Contents

1. [Version performance history, as published 2026-05-29 (v4.0.8 to v4.1.0)](#1-version-performance-history-as-published-2026-05-29)
2. [W7 startup change (v4.0.9 to v4.1.0)](#2-w7-startup-change-v409-to-v410)
3. [v4.0.1 to v4.0.8 release train: component microbenchmarks](#3-v401-to-v408-release-train-component-microbenchmarks)
4. [v4.0.0: v3.0.0 to v4.0.0 full-corpus comparison](#4-v400-v300-to-v400-full-corpus-comparison)

---

## 1. Version performance history, as published 2026-05-29

> **As published 2026-05-29. Raw data not retained. Not reproducible. Different
> cohort and build from any current measurement. In-sample PGO.** Do not compare
> these rows with the README's current table.

The table below is reproduced verbatim from the README of that date.

Per-process latency ratio vs SQLite 3.53.1 — 294-case medium parity benchmark,
882 samples (294 cases × 3 reps), memory profile. Binary: release + fat LTO;
v4.0.9 and v4.1.0 additionally PGO-optimized (quick training set, `clang-18`).

| Version | Median ratio | p95 ratio | Δ median | Δ p95 | Key change |
|---------|:-----------:|:--------:|:-------:|:-----:|------------|
| v4.0.8 | 1.846× | 2.429× | — | — | Release baseline |
| v4.0.9 | 1.780× | 1.990× | −3.6% | −18.1% | PGO quick-training added |
| **v4.1.0** | **1.749×** | **1.887×** | **−1.7%** | **−5.2%** | W7: cgroup-walk bypass |

_Cumulative v4.0.8 → v4.1.0: median **−5.3%**, p95 **−22.3%**._

What is known about these rows:

- **No raw data.** The table was added in commit `057c6cdea` together with the
  version bump. That commit added no raw records, command line or binary digest;
  the only other copy of the numbers is `CHANGELOG.md`.
- **Lane.** The claimed lane was `scripts/perf/medium.sh`, deleted in
  `0f831a2bb`. It used a custom Python replay driver, not `redline-testing run`,
  with 2 workers, `taskset 2-5`, `REDLINEDB_DEFAULT_DURABILITY=normal` and
  `/dev/shm`.
- **Cohort.** The 294 case ids (restored with their digest in
  [`bench/perf/cases/medium-set.txt`](../bench/perf/cases/medium-set.txt)) were
  picked to over-represent slow cases, which favors later versions through
  regression to the mean. 289 are `:memory:` cases and 5 use a temp file, so
  "memory profile" is not exact.
- **In-sample PGO.** At `057c6cdea`, `scripts/perf/pgo.sh` defaulted to
  `--training-subset medium`, the same 294 cases it was then measured on. The
  "quick" set named above also overlaps the medium set heavily. Either way the
  v4.0.9 and v4.1.0 builds were trained on the cases they were measured on, and
  the change from v4.0.8 mixes a build change with code changes.
- **Version identity.** "v4.1.0" here means commit `057c6cdea`. The `v4.1.0`
  tag (`af2082631`) is 40 commits later.
- **Noise.** Four runs of one build on the related quick-set lane returned
  1.685×, 1.786×, 1.791× and 1.812× (see
  `docs/archive/AGENT_CHAT.full-through-2026-05-28T1355Z.md`). The 1.7%
  v4.0.9 → v4.1.0 step is well inside that spread.
- **No match found.** Restricting the committed datasets to these 294 ids does
  not reproduce any row exactly.

## 2. W7 startup change (v4.0.9 to v4.1.0)

> **Historical, as published.** Part of this change was reverted before the
> `v4.1.0` tag, and the syscall count below was never measured.

The following is the README's former "What's new in v4.0.9 → v4.1.0" section,
verbatim:

> **W7** eliminates unnecessary syscalls from the in-memory database startup path.
> Every process invocation of `redlinedb` previously walked the Linux cgroup
> hierarchy to detect CPU parallelism — even for volatile (in-memory) databases
> that don't need it. v4.1.0 fixes both call sites.
>
> | Change | Detail |
> |---|---|
> | `EngineConfig::default()` | No longer calls `cached_available_parallelism()`. Volatile DBs use fixed shard defaults; persistent DBs call `with_detected_parallelism()` inside `Engine::create_inner`. |
> | `BufferPool::new_with_parallelism()` | New cgroup-walk-free constructor; volatile path uses it directly with a derived hint. |
> | `Engine::create_inner` split | Volatile databases skip `create_dir_all` (caller already did it), skip the cgroup walk, and get a lean shard layout. |
>
> **Startup overhead removed per process:** ~6 syscalls (`openat /proc/self/cgroup` + walk of `/sys/fs/cgroup/.../cpu.max`).

Corrections:

- Commit `a8460bb63` ("remove with_detected_parallelism override from
  create_inner"), which is in the `v4.1.0` tag, removed the
  `with_detected_parallelism()` call from `Engine::create_inner` for persistent
  databases. The first table row no longer describes the tagged code.
- "~6 syscalls" is an estimate, not a measurement. On a process that takes
  about 3 ms, it is too small to see in the ratio above.

## 3. v4.0.1 to v4.0.8 release train: component microbenchmarks

> **Component microbenchmarks, not SQL query speedups.** The 14.4× and 194×
> figures compare one internal code path with another. They are not latency
> ratios against SQLite and say nothing about end-to-end SQL speed.

The README's former "What's new in v4.0.1 → v4.0.8" section, verbatim:

> **Phase 5** (v4.0.1) shipped 20+ workstreams across five waves — median ratio vs SQLite **1.904× → 1.857×**, cases ≥ 2.0× slower **193 → 60 (−69%)**. **Phase 6** (v4.0.2 → v4.0.8) ships eight further releases — full per-version detail in [CHANGELOG.md](../CHANGELOG.md). Highlights:
>
> | Release | Work-stream | Headline |
> |---|---|---|
> | v4.0.4 | R2 — ScalarProgram VM dispatch + parallel-scan kernel API + AccessPath IR planner wiring | +55 tests; PRAGMA toggles for opt-in |
> | v4.0.5 | R3-B — per-PreparedStatement VM compile cache | +11 tests; thread-local scoped cache |
> | v4.0.6 | R3-C + R4-A — SQL-side parallel-scan dispatch + Morsel hash-aggregator | +21 tests; AVX2 SUM(i64) **14.4× speedup** vs scalar |
> | v4.0.7 | R4-B — WAL group-commit pipeline (`wal_pipeline` feature) | **194× WAL throughput speedup**, 250× syscall reduction |
> | v4.0.8 | R3-A — `PRAGMA redline_scalar_vm` + `PRAGMA redline_planner_use_access_path` | SQL surface for the R2-A/R2-C toggles |
>
> Workspace test count: **1786 → 1990 (+204)** with zero regressions. SIMD wins gated behind runtime `is_x86_feature_detected!` dispatch + the `unsafe-ledger.toml` audit; WAL group-commit and parallel-scan dispatch are feature-flagged so default builds remain byte-identical to v4.0.3.

What the headline numbers actually measured:

- **14.4×** is the AVX2 `SUM(i64)` kernel against the scalar loop on one
  1024-row morsel, built with `target-cpu=native`. See `CHANGELOG.md` [4.0.6].
- **194×** is batched `writev` plus one `fdatasync` per batch against one
  `fdatasync` per record, for 10,000 records of 256 bytes, under the non-default
  `wal_pipeline` feature. The pipeline is not wired into `Database`, so no SQL
  write goes through it. See `CHANGELOG.md` [4.0.7].
- **1.904× → 1.857×** was measured on the older 1127-case corpus with 10
  workers. It is a different corpus and lane from sections 1 and 4.
- "Default builds remain byte-identical to v4.0.3" was never checked by a
  recorded command, and later releases changed the default build.
- The workspace test counts were not recorded with a command and cannot be
  checked.
- "Full per-version detail in CHANGELOG.md" is not accurate: `CHANGELOG.md`
  has entries for v4.0.1 and v4.0.3 to v4.0.7, but none for v4.0.2, v4.0.8 or
  v4.0.9.

## 4. v4.0.0: v3.0.0 to v4.0.0 full-corpus comparison

> **Historical.** Measured 2026-05-25 with 30 parallel workers on a shared
> 128-core host, no CPU pinning, `target-cpu=native` builds, and the
> then-external `redline-testing v1.0.0` runner. The raw records are committed,
> and the summary recomputes from them, but the lane is not a pinned benchmark
> and the pass counts come from the old, lenient comparator.

**Phase 0-4 SQLite-parity speed-gap closure.** Fourteen named optimizations
across the build profile, parser, scalar fast paths, and CTE/aggregate/window
hot paths, measured against the `redline-testing v1.0.0` parity harness on the
full 2445-case `sqlite_parity` suite. Median per-case latency ratio against
SQLite improved from **1.837× → 1.738×** with **zero parity regressions**
(identical pass set in v3.0.0 and v4.0.0: **2374 passed + 67 failed + 4 skipped
= 2445**; the 67 failures were pre-existing edge cases in `typeof()` reporting,
IEEE-754 last-digit precision, fullwidth Unicode case-folding, BLOB hex
encoding, and `AUTOINCREMENT` semantics).

> **Note on corpus size.** The redline-testing official corpus had grown from
> 1127 cases (prior CI snapshot) to 2445 cases in v1.0.0. The v4.0.0 numbers in
> this section are that release's measurement. They were produced before the
> comparator enforced exit codes, error text and byte-exact output, so the pass
> counts are not comparable with v5.0.0's.

### Per-case latency distribution — RedlineDB / SQLite ratio (full 2445-case corpus, passed cases only)

| Bucket | v3.0.0 (main) | v4.0.0 | Delta |
|---|---:|---:|---:|
| `< 1.0×` (RedlineDB faster than SQLite) | 7 | 8 | **+1** |
| `1.0 – 1.2×` | 16 | 28 | **+12** |
| `1.2 – 1.5×` | 173 | 292 | **+119** |
| `1.5 – 2.0×` | 1622 | 1748 | **+126** |
| `2.0 – 3.0×` | 555 | 297 | **−258** |
| `≥ 3.0×` (tail outliers) | 1 | 1 | 0 |
| **Total** | **2374** | **2374** | 0 |

258 cases moved out of the `2.0–3.0×` slow band; 119 moved into the `1.2–1.5×`
band. Per-case: **1410 cases (59.4%) are ≥5% faster** in v4.0.0, 386 (16.3%)
are ≥5% slower, 578 (24.3%) within ±5% noise. Mean per-case target-latency
change: **−6.85%** (median **−7.67%**).

That README also said the Jankurai code-health score "holds at **85/100
(pass)**", and its detail section read "85 / 100 — `pass` (advisory)".
Code-health scoring is contributor tooling and is no longer reported with
releases.

### Named optimizations shipped

| Phase | Commit | Optimization |
|---|---|---|
| 1.1 | `f8ed61f` | fat LTO + `opt-level=3` + `target-cpu=native` release profile |
| 1.2 | `b62d4ad` | parser rewrite-pass allocation elimination |
| 1.3 | `4a89e9a` | borrow + stack-buffer function-name lowercase |
| 1.4 | `b229f90` | cache + lighten `/dev/shm` writability probe |
| 1.5 | `2e13dc5` | fromless `SELECT` fast path |
| 1.6 | `a20de92` | `ahash::RandomState` for `StatementCache` |
| 2.1–2.2 | `5bbe650` | ASCII fast paths for `LENGTH`/`UPPER`/`LOWER` + `memmem` for `INSTR` |
| 2.3+2.5 | `9abab6c` | `value_as_str` + hot scalar fn migration to `Cow` |
| 2.4 | `32e078d` | `itoa` for streaming i64 CLI output |
| 4.1 | `efc9a6e` | fromless-SELECT walker covers `sqlparser` scalar variants |
| 4.2 | `d348e0b` | dedup aggregate cache key + reuse fn-name lower |
| 4.3 | `e569d6c` | capacity hints in per-row hot allocations |
| 4.4 | `32200c2` | hoist CTE lowercase out of recursive iteration loop |
| 4.5 | `2f21ea3` | reuse scratch buffer for window partition keys |

The release profile no longer sets `target-cpu=native`; release builds now
target baseline x86-64, and CPU specialization needs an explicit `RUSTFLAGS`.

### Benchmark provenance

- **Harness:** `redline-testing v1.0.0`, then an external repository. The runner now lives in-tree at [`subrepos/redline-testing`](../subrepos/redline-testing).
- **SQLite reference:** `sqlite3 3.53.1` (release build, SHA-256 `fd3bdd25217a849f8f4fa295fb78199cfd69b0c4d47ba8d8c32a1aa328bd147e`).
- **Workload:** full `sqlite_parity` suite — 2445 cases × 3 measured reps + 1 warmup, **`--workers 30`** on a 128-core Linux x86_64 host, no CPU pinning.
- **Target binary (v4.0.0):** SHA-256 `7ae60cb513e866b4a94996968b0c6b9f01b0071776bc842f526702be33f05e56` (release profile, fat LTO, `target-cpu=native`).
- **Baseline binary (v3.0.0):** SHA-256 `da770dfd25beeb36aa22f8ce7a09d935b4e9fd7c8b2a77c36e621c46cec69ef2`.
- **Raw JSONL evidence (committed):** [`benchmark-results/sqlite-parity/perf-baselines/v3.0.0-baseline.jsonl`](../benchmark-results/sqlite-parity/perf-baselines/v3.0.0-baseline.jsonl), [`v4.0.0-baseline.jsonl`](../benchmark-results/sqlite-parity/perf-baselines/v4.0.0-baseline.jsonl), and the structured A/B summary [`v3-vs-v4-summary.json`](../benchmark-results/sqlite-parity/perf-baselines/v3-vs-v4-summary.json).
- **Reproduce (as published).** This command used the historical runner and the `scripts/perf/full.sh` of that date. The script has since changed: it now refuses an incomplete run and writes each run to its own directory.
  ```bash
  cargo build --release -p redlinedb-cli
  PERF_WORKERS=30 \
    REDLINE_TESTING_BIN=/path/to/redline-testing \
    SQLITE_REF_BIN=/path/to/sqlite3-3.53.1 \
    scripts/perf/full.sh target/release/redlinedb v4.0.0-final
  ```
