# Parity closure plan — 2026-09-22

Every number here was produced on xbabe2 on 2026-09-22 against `main` at `4a26fb75c`,
with an oracle configured to match CI exactly. Commands are in "Reproducing the
measurements" at the end. Where a claim is not measured, it says so.

## Where we actually are

| Lane | Result | Denominator |
|---|---|---|
| `sqlite_parity` (disk profile) | **2441 pass · 0 fail · 4 capability-gated skip** | 2445 |
| `memory` (same corpus, memory profile) | **2441 pass · 0 fail · 4 capability-gated skip** | 2445 |
| `rql_phase1` | **1129 pass · 0 fail · 256 skip** | 1385 |
| `beyond_sqlite`, in scope | **133 pass · 15 fail** | 148 |
| `beyond_sqlite`, whole corpus | 162 pass · 103 fail | 265 |
| jankurai, six repos | 86–90, **0 caps, 0 hard findings**, 32 medium | min 85 |

The whole-corpus beyond-Postgres figure reproduces
`metadata/beyond_sqlite/postgres-regression.json` **exactly** — the same 103 case ids,
no additions, no omissions. That is the evidence standard this document holds itself to.

### The SQLite lane is closed

Against the reference CI actually builds, both SQLite profiles pass every case they
attempt. The four skips are explicit capability gates, not silent passes:

```
00093 CREATE_VIRTUAL_TABLE_FTS5_OPTIONAL   lacks fts5 virtual table
00094 FTS5_HIGHLIGHT_OPTIONAL              lacks fts5 virtual table
00095 CREATE_VIRTUAL_TABLE_RTREE_OPTIONAL  lacks rtree virtual table
00096 DBSTAT_OPTIONAL                      lacks dbstat virtual table
```

This is a real result but a **fragile** one, for the reason in §1 below. Read that
before quoting the number.

---

## 1. The reference build is the load-bearing risk (P0, S)

Five cases — `00166`, `00219`, `00220`, `11437`–`11439` — **change verdict depending on
which `sqlite3` you compare against**, and nothing in the tree stops the wrong one being
used.

| build | SOUNDEX | `UPDATE … ORDER BY LIMIT` | SESSION |
|---|---|---|---|
| CI canonical (`scripts/sqlite/build-reference.sh`) | no | **no** (see below) | yes |
| local `target/sqlite-reference/3.53.1` | yes | yes | no |
| local `target/sqlite-reference/3.53.1-ordinary` | no | no | no |

Measured: 8 failures against the local feature-rich build, **0** against the canonical one.

**1a. The canonical prefix has no stamp.** `existing_shell_is_current()` requires
`$prefix/.sqlite-reference-sha3`. Neither local prefix has one, so the next
`just redline-testing-official` run rebuilds into `target/sqlite-reference/3.53.1/` and
**silently overwrites the feature-rich build** that other evidence refers to. Fix: build the
canonical reference into its own stamped prefix and make every lane resolve the binary
through `build-reference.sh` rather than by path.

**1b. `PRAGMA compile_options` lies, and it is upstream's lie, not ours.** The canonical
build reports `ENABLE_UPDATE_DELETE_LIMIT` in `PRAGMA compile_options`, and then:

```
sqlite> DELETE FROM t ORDER BY a LIMIT 1;
Parse error near line 1: near "ORDER": syntax error
```

The autoconf amalgamation ships a **pre-generated `parse.c`**. Defining
`SQLITE_ENABLE_UPDATE_DELETE_LIMIT` at compile time sets the reported option but cannot add
the grammar, which lemon must emit from `parse.y`. Any conclusion drawn from
`compile_options` about the reference's *grammar* is unsound. Record this in
`docs/sqlite-parity.md` so it is not rediscovered a third time.

**Consequence for `crates/sql/src/parser/rewrite/dml_limit.rs`:** the 403-line rewrite
staying default-OFF is **correct**, and `json_dispatch.rs:216-219` rejecting `soundex()`
with "no such function" is **correct**. Both match the canonical reference. Neither should
be flipped without also changing the reference, and the comments at both sites should say
"the canonical reference cannot parse this" rather than "the official build does not define
the macro", which is the misleading half.

---

## 2. Beyond-Postgres: 15 in-scope failures, fully classified

Every one reproduced against `postgres@sha256:efdf07c2…`, `C`/`C` collation, UTC — CI's
pinned image and settings. **Collation matters:** running the oracle at `en_US.utf8`
produces 107 failures instead of 103, and `20058 ILIKE_NON_ASCII_DOES_NOT_FOLD` appears as
a failure that does not exist. An oracle that is not `C` is not evidence.

### 2a. Group A — blocked only by the comparator (4 cases)

`oracle.rs:360` passes a case only when `ref_norm == tgt_norm && reference.exit_code == 0`.
When both engines reject the statement identically it returns

> `matching nonzero exit is not a semantic pass`

**Eleven of the fifteen in-scope failures are unpassable by construction.** The guard is
right in intent — RedlineDB answering "I don't implement DROP TYPE" must never count as
agreeing with Postgres's "invalid enum value" — but it is absolute, so cases whose *entire
point* is "both engines reject this" can never pass.

Four cases reject for the correct reason and are blocked only by this:

| case | RedlineDB's error | correct? |
|---|---|---|
| `20136` GENERATED_ALWAYS_REJECTS_EXPLICIT | `cannot insert a non-DEFAULT value into column "id"` | yes |
| `20247` GENERATED_ALWAYS_AS_IDENTITY_REJECTS_EXPLICIT | same | yes |
| `20203` ALTER_COLUMN_SET_NOT_NULL | `NOT NULL constraint failed: mig_set_notnull.name` | yes |
| `20103` MERGE_NOT_MATCHED_BY_SOURCE_PG17 | `MERGE WHEN NOT MATCHED BY SOURCE (PG17+) is not supported` | yes, on PG16 |

**The fix — declare the expected message in the corpus, never infer it.**

Add an optional `expected_target_stderr_contains` to the beyond-SQLite manifest schema. A
matching non-zero exit becomes a pass **only when both** hold:

1. the case declares `expected_target_stderr_contains` and the target's stderr contains it; and
2. the target's `setup_stdin`, executed as its own invocation, exited 0.

A case that declares nothing keeps failing exactly as today, so **no case flips silently**.
Condition 2 is what keeps `20021` and `20023` honest: both die in *setup* on `DROP TYPE` /
`DROP DOMAIN`, never reaching the statement under test. Condition 1 is what keeps `20308`,
`20340`, `20418`, `20429`, `20438` honest: their errors are capability gaps
(`unsupported sql: …`), and a corpus author cannot write those down as the semantic
rejection without the diff making it obvious.

`setup_stdin` and `stdin` are already separate manifest fields, so condition 2 needs no
corpus change — only a second target invocation.

**Acceptance — state it before running.** After this change: `20136`, `20247`, `20203`,
`20103` pass; `20021`, `20023`, `20308`, `20340`, `20418`, `20429`, `20438` **must still
fail**. If any of those seven passes, the guard is wrong and the change must be reverted,
not adjusted. In-scope becomes **137/148**.

Effort: M — one comparator change, four corpus annotations, one schema field.

### 2b. Group B — RedlineDB rejects for the wrong reason (7 cases)

These need engine work, not harness work. They are correctly failing today.

| case | blocked on |
|---|---|
| `20021` ENUM_REJECTS_BAD_VALUE | `DROP TYPE` in setup |
| `20023` DOMAIN_REJECTS_NEG | `DROP DOMAIN` in setup |
| `20308` PLPGSQL_RAISE_EXCEPTION | `DO $$ … $$` not parsed |
| `20340` VECTOR_EXTENSION_PROBE_UNAVAILABLE | `CREATE EXTENSION` not parsed |
| `20418` LOGICAL_SLOT_REJECTS_WAL_LEVEL_REPLICA | `pg_create_logical_replication_slot` |
| `20429` LOGICAL_SLOT_PEEK_REQUIRES_SLOT | table-valued functions |
| `20438` LISTEN_INVALID_CHANNEL_NAME_ALL | `LISTEN` not parsed |

`20021` and `20023` are the cheapest: a `DROP TYPE`/`DROP DOMAIN IF EXISTS` that is a
no-op over a catalog with no types or domains is honest — the object genuinely is not
there — and unblocks the statement under test. The other five want a real feature; leave
them failing and visible rather than annotating them into a pass.

### 2c. Group C — genuine behavioural divergences (4 cases)

| case | reference | RedlineDB | fix | size |
|---|---|---|---|---|
| `20111` DISTINCT_ON_REQUIRES_ORDER_BY | errors | returns `1\|a` | **missing validation** — `SELECT DISTINCT ON` must require a matching leading `ORDER BY` (`parser/select.rs`) | S |
| `20255` SAME_NAMED_TABLES_DIFFERENT_SCHEMAS | 4 rows | `kernel error: object already exists` | schema namespacing; currently mangled in `parser/rewrite/pg_ddl.rs` | L |
| `20249` SEQUENCE_OWNED_BY | 2 rows | `unsupported DDL expression: nextval(...)` | `nextval()` in a column DEFAULT | M |
| `20106` LATERAL_WITH_SET_RETURNING_FUNCTION | 3 rows | `only direct table scans are supported` | TVFs materialise at bind time, so LATERAL args cannot see outer columns | L |

`20111` is the only one that is a **correctness** bug rather than a missing feature:
RedlineDB accepts a query Postgres rejects. Fix it first regardless of case count.

### 2d. The skip-list is not a filter, and 29 entries are stale

`metadata/beyond_sqlite/skip-list.toml` has 117 entries with per-case rationale. **No code
reads it** — `grep -rn skip-list` across `*.rs`, `*.sh`, `*.yml`, `*.just` returns nothing
outside documentation. "148 in scope" is a subtraction performed by hand in every report
that quotes it, which is why the figure drifts between documents.

Worse, **29 of the 117 now pass**:

```
20028 20041 20043 20044 20045 20046 20047 20048 20049 20243 20256 20257
20341 20342 20406 20407 20408 20410 20411 20415 20416 20417 20422 20431
20432 20435 20436 20443 20444
```

Each is marked `target_release = "deferred"` — "no plan to revisit" — and each is green.
That is closed work being reported as abandoned.

Two changes, both S:
1. Make the runner **read** `skip-list.toml` and emit `in_scope_passed` / `in_scope_failed`
   / `deferred` beside the raw counts, so no one subtracts by hand again.
2. Add a check that **fails** when a deferred case passes. A skip-list entry that has come
   true is a bug in the skip-list; it should be as loud as a regression.

---

## 3. RQL phase-1: 256 skips, now legible

`{err}` → `{err:#}` (PR #91) turned 142 identical "lower SQL to RQL for `<id>`" strings into
104 distinct causes. Full breakdown of all 256:

| n | theme | verdict |
|---:|---|---|
| 80 | hard-coded "known runtime divergence" list | **re-validate — likely mostly stale** |
| 44 | `SUBSTR`/`SUBSTRING` AST node | **close** |
| 24 | JSON `->` / `->>` operators | **close** |
| 18 | expected-error cases | leave — correct for a latency suite |
| 16 | sqlparser cannot parse | upstream |
| 11 | compound / derived / `VALUES` query body | IR ceiling |
| 10 | savepoints | IR ceiling |
| 9 | function/operator detail (`LIKE ANY`, arg clauses) | mixed |
| 8 | `CEIL`/`FLOOR` AST node | **close** |
| 5 | join constraints (`NATURAL`, `USING`) | M |
| 4 | `COLLATE` expression | M |
| 4 | DDL detail (table constraints, generated cols, partial index) | IR ceiling |
| 3 | `RETURNING` | IR ceiling |
| 3 | view / trigger | IR ceiling |
| 3 | `PRAGMA` | IR ceiling |
| 2 | `INSERT … SELECT` / CTAS | IR ceiling |
| 2 | `TRIM` AST node | **close** |
| 1 | `CASE` expression | S |
| 9 | singletons (table factor, CHECK constraint, …) | triage |

**3a. The 54 AST-node cases are unfinished work, not scope (S–M, 54 cases).**
`SUBSTR` (44), `CEIL`/`FLOOR` (8), `TRIM` (2) fail only because sqlparser returns dedicated
AST variants that `expr()` has no match arm for. They lower to `RqlExpr::Function`, which
already works — **no IR change**. `docs/rql.md:20` lists functions as in scope, so these are
simply not done.

**3b. Re-validate the 80-entry divergence list (S, up to 80 cases).** Six of six sampled
previously agreed byte-for-byte. Delete the list, re-run, re-add only what still diverges,
with the observed diff recorded per entry. Note `11437`–`11439` sit on this list *and* are
real SOUNDEX gaps — the same thing suppressed twice.

**3c. The 24 JSON-operator cases (M).** `->` and `->>` are the last common operators
outside phase 1 and the largest single non-function cluster.

**3d. Write `docs/rql-phase1-scope.md` (S).** There is no RQL equivalent of
`docs/beyond-postgres-skips.md`. Until there is, "deferred" and "not yet done" are the same
string in the output, which is exactly the ambiguity that let the 80-entry list go stale.
The real boundary is the IR: `RqlStatement` has 11 variants (no savepoint, view, trigger,
CTAS, `RETURNING`, `INSERT…SELECT`, `PRAGMA`) and `RqlSelect` has no `with`, compound, or
derived table. Roughly 40 of the 256 are genuinely IR-ceilinged; say so per case.

---

## 4. Reporting honesty

**4a. `beyond-sqlite-summary.json` leads with the wrong number.** Today's artifact:

```json
"passed_features": 269, "failed_features": 0, "coverage_pct": 97.11,
"target_passed": 162, "target_failed": 103
```

`coverage_pct: 97.1` and `failed_features: 0` describe the **psql↔psql oracle
self-compare**. A reader concludes 97% while RedlineDB is at 61% whole-corpus / 90%
in-scope. Lead with `target_*`, rename the oracle figures `oracle_self_compare_*`, and add
`in_scope_passed` / `in_scope_failed` from §2d. S, and it should go first — every other
number in this document is quoted from these artifacts.

**4b. Provenance has two defects.** `source_commit` is `null` and
`started_unix_ms == ended_unix_ms` on an 85-second run. Both make an evidence artifact
unattributable to a commit or a duration. S.

**4c. The required parity gate prints no counts.** The whole of
`parity (redline-testing-official)` on a passing run is:

> `Verified SQLite and PostgreSQL corpus report generation passed.`

The Postgres side *is* genuinely ratcheted against `postgres-regression.json` (103 pinned
ids) — that mechanism is sound and should be credited. But the job should print
pass/fail/skip per suite so a reader of the log knows what was verified. S.

**4d. The SQLite comparator never reads `expected_stdout`.** `runner.rs:268-292` is purely
differential: reference vs target. All 2,445 pinned expectations are decorative in that
lane, and two mutually-consistent-but-wrong outputs pass. The fixture-validating comparator
exists in the packaged `dist/` tarball and found 296 failures on this corpus. The staged
`slice3-strict-comparator` patch (251 lines, applies clean to `main`) restores it in-tree.
**L, and it will go red** — schedule it deliberately, after §1a pins the reference, and
triage its output into fix / delete / `#[ignore]` with a reason string.

---

## 5. jankurai: all six repos pass, 32 medium findings remain

| repo | score | caps | hard | medium |
|---|---:|---:|---:|---:|
| redline-web | 90 | 0 | 0 | 2 |
| redline | 87 | 0 | 0 | 6 |
| redline-central | 87 | 0 | 0 | 4 |
| redline-split-ops | 87 | 0 | 0 | 5 |
| redlineDB | 86 | 0 | 0 | 10 |
| redline-testing | 86 | 0 | 0 | 5 |

The "above 85, no caps" bar is met everywhere. Cheapest remaining movement:

- **`redline`: three `HLT-047-CANONICAL-README` findings** — README missing an AGENTS.md
  link, a quick-start, and a status badge. Half of that repo's findings, XS.
- **`redlineDB`: seven `HLT-046-UNNECESSARY-VARIETY`** — `Cell`, `Error`, `Op`, `Step`,
  `AggKind`, `JoinKind`, `AccessPath` each defined with diverging shapes in two modules.
  Genuine duplication worth collapsing; treat as refactoring, not audit-chasing.
- **`HLT-001-DEAD-MARKER`** on `crates/sql/src/parser/pragma.rs` (1951 LOC) and
  `redline-testing/src/evidence.rs` (666 LOC).

---

## 6. Sequence

Ordered so that nothing is measured against a surface that later moves.

| # | item | § | size | closes |
|---:|---|---|---|---|
| 1 | Pin and stamp the canonical reference; resolve it via `build-reference.sh` everywhere | 1a | S | protects all 2445 |
| 2 | Record the `compile_options`-vs-grammar trap in `docs/sqlite-parity.md` | 1b | XS | — |
| 3 | Summary/provenance headline: `target_*` first, `in_scope_*`, `source_commit`, real duration | 4a,4b | S | — |
| 4 | Runner reads `skip-list.toml`; fail when a deferred case passes; retire the 29 stale | 2d | S | 29 reclassified |
| 5 | `DISTINCT ON` requires `ORDER BY` | 2c | S | 1 (correctness) |
| 6 | `expected_target_stderr_contains` + setup-phase guard | 2a | M | 4 → **137/148** |
| 7 | `DROP TYPE` / `DROP DOMAIN IF EXISTS` as honest no-ops | 2b | S | 2 → **139/148** |
| 8 | RQL: `SUBSTR`/`CEIL`/`FLOOR`/`TRIM` match arms | 3a | S–M | 54 skips |
| 9 | RQL: re-validate the 80-entry divergence list | 3b | S | ≤80 skips |
| 10 | `docs/rql-phase1-scope.md` | 3d | S | — |
| 11 | RQL: JSON `->` / `->>` | 3c | M | 24 skips |
| 12 | `nextval()` in column DEFAULT | 2c | M | 1 → 140/148 |
| 13 | Parity gate prints per-suite counts | 4c | S | — |
| 14 | `slice3-strict-comparator` — fixture validation in-tree | 4d | L | expect red |
| 15 | Schema namespacing; LATERAL/TVF | 2c | L | 2 → 142/148 |

Steps 1–7 are two to three days and take beyond-Postgres in-scope from **133/148 to
139/148 (94%)** with the SQLite lane protected. Steps 8–11 take RQL skips from 256 to
roughly 100. Step 14 is the one that is *supposed* to hurt; it belongs after everything
above it is stable.

## Not worth doing, stated so it stops being re-proposed

- **A real FTS5 engine** (2–4 months) or **R-tree index**. Cases `00093`–`00095` are already
  satisfied *by output* through the undocumented "FTS-lite" `MATCH`-as-substring path, but a
  DDL shim that maps `USING fts5` to an ordinary table must be documented as
  "vtab-shaped DDL alias" and never as "we implemented FTS5". A capability-gated skip is more
  honest than a shimmed pass.
- **A faithful `dbstat`** (`00096`) at any effort: RedlineDB's page format is not SQLite's,
  so every `dbstat` row would be fiction.
- **The 18 RQL expected-error skips** — correct for a latency suite. Worth one line in
  `docs/rql-phase1-scope.md` noting that nothing else checks RQL rejects what SQL rejects.
- **GLOB** (9 RQL cases) — blocked upstream in sqlparser.
- **Flipping `dml_limit` on or implementing `soundex()`** — both would *break* parity
  against the canonical reference. See §1b.

## Standing caveats

- **Local reference builds are not CI's.** This is what made PR #80's first revision wrong.
  A local parity run is a signal; `parity (redline-testing-official)` is the proof. The
  canonical build at `target/sqlite-reference/3.53.1-canonical` was made specifically to
  close this gap for local work, and §1a is about making that the default rather than a
  thing each agent remembers.
- **An oracle that is not `C`/`C` is not evidence.** `en_US.utf8` silently produces four
  extra failures.
- **`status: "passed"` because `failed == 0` while skips are free** is the false-green
  jankurai's own `false-green-test-risk` exists to catch. Every count in this document is
  quoted as pass / fail / **skip**.

## Reproducing the measurements

```sh
# CI-canonical SQLite reference (stamped, separate prefix)
REDLINEDB_SQLITE_REFERENCE_PREFIX="$PWD/target/sqlite-reference/3.53.1-canonical" \
  bash scripts/sqlite/build-reference.sh

# CI-pinned Postgres oracle: same image digest, C collation, UTC
docker run -d --name redline-pg-oracle \
  -e POSTGRES_DB=redlinedb_beyond -e POSTGRES_USER=redlinedb -e POSTGRES_PASSWORD=postgres \
  -e POSTGRES_INITDB_ARGS="--locale=C --encoding=UTF8" -p 127.0.0.1:55444:5432 \
  postgres@sha256:efdf07c2f9d4df592783dcc8ea5f6db02efbf5f6452b527225ff5e58364570e9
docker exec redline-pg-oracle psql -U redlinedb -d redlinedb_beyond \
  -c "ALTER DATABASE redlinedb_beyond SET TimeZone TO 'UTC'"
# provenance must read exactly: 160015|C|C|UTC

for s in sqlite_parity memory rql_phase1 beyond_sqlite; do
  PGOPTIONS='-c client_min_messages=warning' \
  REDLINE_TESTING_POSTGRES_IMAGE="sha256:efdf07c2f9d4df592783dcc8ea5f6db02efbf5f6452b527225ff5e58364570e9" \
  REDLINE_TESTING_POSTGRES_URL="postgres://redlinedb:postgres@127.0.0.1:55444/redlinedb_beyond" \
  subrepos/redline-testing/target/release/redline-testing run --suite "$s" \
    --target-bin target/release/redlinedb \
    --sqlite-bin target/sqlite-reference/3.53.1-canonical/bin/sqlite3 \
    --tmp-root /dev/shm/redline --output "$OUT/$s.jsonl" --progress never
done
```

`PGOPTIONS=-c client_min_messages=warning` is required: Postgres `NOTICE` lines otherwise
break the oracle's self-compare on 53 cases whose stdout is identical.

jankurai writes into generated zones by default. Always redirect both outputs to scratch:

```sh
jankurai score . --json /path/to/scratch/repo.json --md /path/to/scratch/repo.md --no-score-history
```

