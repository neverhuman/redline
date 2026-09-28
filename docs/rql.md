# RQL

RQL is the Redline Query Language. In v0.1 it is an additive, default-off typed
relational IR for RedlineDB, not an alternate SQL keyword syntax.

SQL remains the compatibility frontend. RQL callers submit a `RqlProgram` or
`RqlStatement` through Rust APIs, or JSON through `redlinedb --rql <db>`.
The RQL path lowers directly into existing prepared templates and executor
plans. It does not render SQL text and does not round-trip through the SQL
parser.

## Rust APIs

The public API surface is:

- `Connection::prepare_rql(&RqlStatement)`
- `Connection::execute_rql(&RqlProgram)`
- `Database::prepare_rql(&RqlStatement)`

The typed IR covers the phase-1 relational surface: create/drop table and
index, insert values, update, delete, select, filters, joins, grouping,
ordering, limits, scalar expressions, functions, casts, and simple subqueries.
Table constraints and other statements outside that surface are not expressible
yet; the conformance section below counts them.

## CLI

`redlinedb --rql <db>` reads RQL JSON from stdin and renders query output
through the normal CLI output modes. `--rql` cannot be combined with `--cmd` or
SQL arguments.

```json
{
  "statements": [
    {
      "type": "create_table",
      "table": { "name": "items" },
      "columns": [
        { "name": "id", "declared_type": "INTEGER", "primary_key": true },
        { "name": "label", "declared_type": "TEXT" }
      ]
    },
    {
      "type": "insert",
      "table": { "name": "items" },
      "columns": ["id", "label"],
      "values": [[
        { "type": "integer", "value": 1 },
        { "type": "text", "value": "Ada" }
      ]]
    },
    {
      "type": "select",
      "projection": [
        {
          "type": "expr",
          "expr": { "type": "column", "column": { "name": "label" } }
        }
      ],
      "from": { "name": { "name": "items" } }
    }
  ]
}
```

Piping that program into `redlinedb --rql :memory:` prints `Ada`.

## Conformance

`redline-testing run --suite rql_phase1` keeps the SQLite parity SQL suite
unchanged. It runs SQLite against the original SQL case and RedlineDB against
the generated RQL target input, producing the same JSONL evidence format used
by the existing benchmark/report pipeline. The official lane
(`just redline-testing-official`) runs it with the other suites.

In the v5.0.0 official evidence (`55637d088`, 2026-09-28),
`rql_phase1` passed 1183 of 1385 cases, failed
0, and skipped 202. Every skip is declared in advance
in
[`scope-policy.json`](../subrepos/redline-testing/corpus/sqlite_parity/scope-policy.json),
with a reason and an expiry date, and a skip that is not listed there fails its
case. The skips are of three kinds:

- 105 cases the phase-1 rewriter cannot parse or lower to RQL
  (for example table constraints, savepoints, views, the JSON `->` and `->>`
  operators, and derived tables).
- 77 cases where RQL's output is known to differ from
  SQLite's (reason "known RQL/SQLite runtime-output divergence for phase-1",
  for example 10514 `AVG_DISTINCT_BASIC` and 11001 `MATH_ACOS_0_0`). These are
  declared as skips, not failures, so the pass count does not show them as
  wrong answers.
- 20 expected-error cases, which the suite does not
  run through RQL.

These counts are a snapshot of that evidence; no renderer keeps them current.

## Historical v4.0.1 local run

> **Historical v4.0.1 local run. Not current evidence.** These figures were
> measured on 2026-05-26 with `redlinedb v4.0.1` and published in the README
> until v5.0.0. Their raw records were never committed, and the two published
> tables contradict each other in the ways listed at the end of this section.
> Do not compare them with current results.

The README carried two versions of this benchmark. Both are reproduced
verbatim.

### First version (README "RQL phase-1 local benchmark")

Measured locally on 2026-05-26 with `redline-testing v1.0.0`,
`redlinedb v4.0.1`, `sqlite3 3.45.1`, release binaries, 1 warmup + 3 measured
repetitions, `--workers 1`:

| Comparison | Scope | Result |
|---|---:|---:|
| RQL phase-1 parity | 1,385 candidates | 1,129 passed, 256 skipped, 0 failed |
| RedlineDB SQL median target latency | 1,129 shared passed cases | 3.596 ms |
| RedlineDB RQL median target latency | 1,129 shared passed cases | 3.419 ms |
| RQL / RedlineDB SQL median target ratio | 1,129 shared passed cases | **0.937×** |
| RQL / RedlineDB SQL aggregate target ratio | 3,387 measured samples | **0.894×** |
| Case movement vs RedlineDB SQL | 1,129 shared passed cases | 620 ≥5% faster, 298 within ±5%, 211 ≥5% slower |
| P0 RQL / RedlineDB SQL median target ratio | 577 shared P0 cases | **0.925×** |
| RQL / SQLite SQL median ratio | 1,129 RQL-passed cases | 1.822× |

The published reproduction commands were:

```bash
cargo build --release -p redlinedb-cli
redline-testing run --suite rql_phase1 \
  --target-bin target/release/redlinedb \
  --sqlite-bin sqlite3 \
  --output target/rql-phase1-bench/rql_phase1.raw.jsonl \
  --tmp-root /tmp/rql-phase1-bench \
  --workers 1 --repetitions 3 --warmup 1 --progress never

# Same target binary, SQL compatibility path, filtered afterward to the
# rql_phase1 case IDs that passed both runs. This full-suite SQL command may
# exit non-zero if unrelated sqlite_parity cases fail in the local environment.
redline-testing run --suite sqlite_parity \
  --target-bin target/release/redlinedb \
  --sqlite-bin sqlite3 \
  --output target/rql-phase1-bench/sqlite_parity.raw.jsonl \
  --tmp-root /tmp/rql-phase1-sql-bench \
  --workers 1 --repetitions 3 --warmup 1 --progress never
```

### Second version (README "RQL Phase 1")

| Metric | Value |
|---|---|
| Cases passed | **1 129 / 1 385** |
| Cases skipped (optional capability) | 256 |
| Cases failed | 0 |

| Metric | RQL / SQL ratio |
|---|---|
| Median per-case latency | **0.937×** (RQL 6.3 % faster) |
| P90 per-case latency | 1.131× |
| P95 per-case latency | 1.192× |
| Aggregate wall-time (1 129 cases) | **0.894×** (RQL 10.6 % faster) |
| Cases where RQL is faster | **800 / 1 129 (70.9 %)** |
| Cases within 5 % of SQL | 298 / 1 129 (26.4 %) |
| Cases ≥ 5 % slower via RQL | 211 / 1 129 (18.7 %) |

**RQL vs SQLite reference:** median per-case ratio **1.822×** (vs SQLite
3.45.1), consistent with the SQL-interface parity gap.

Its provenance said: harness `redline-testing rql_phase1` suite; SQLite
reference `sqlite3 3.53.1` built by `scripts/sqlite/build-reference.sh`;
workload 1 129 passing cases × 3 measured reps + 1 warmup, 10 workers; raw JSONL
at `target/rql-phase1-bench/rql_phase1.raw.jsonl` (CI artifact
`redlinedb-rql-benchmark-evidence`).

### Why these numbers cannot be trusted as published

- **Reference build.** The first version and the second version's SQLite
  ratio name the system `sqlite3 3.45.1`; the second version's provenance names
  the pinned 3.53.1 build. Which binary produced the 1.822× ratio is unknown.
- **Worker count.** The first version says `--workers 1`; the second says 10
  workers. Parallel workers inflate RedlineDB's timings more than SQLite's.
- **Case movement.** The first version's 620 + 298 + 211 = 1,129 is
  consistent. The second version's 800 faster + 298 within 5% + 211 slower =
  1,309, more than the 1,129 cases measured, so its "800" cannot be right.
- **Missing evidence.** No CI job or artifact named
  `redlinedb-rql-benchmark-evidence` exists, and files under `target/` are
  not committed. The raw records are gone.
- **Scope.** The second version described phase 1 as covering "the full DML +
  query surface", while 256 of 1,385 cases were skipped. Why each was skipped
  was not recorded.
- **Different runner and corpus state.** `redline-testing v1.0.0` did not yet
  enforce exit codes, error text or byte-exact output, so its pass counts are
  not comparable with current ones.

A current RQL-versus-SQL timing would need the release bench protocol (one
pinned worker, several interleaved runs, the pinned reference, raw records
committed); none has been published for v5.0.0.
