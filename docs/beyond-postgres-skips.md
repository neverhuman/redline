# Beyond-Postgres record

The execution gate and this document now say the same thing.

The beyond-SQLite corpus is 265 cases, every one `ordered_rows`, compared with
PostgreSQL 16.15 through the SQL shell. On `d5d1c57bb` the regression file
`metadata/beyond_sqlite/postgres-regression.json` has an empty `failed_cases`
list. The README block reports **265 / 265 passed, 0 failed, 0 skipped**.
Hosted `parity (redline-testing-official)` on that tree was green.

A pass is both engines exiting 0, or one of the 12 declared rejections whose
stderr the corpus names. A skip is not a pass. The gate re-derives that rule
in `subrepos/redline-testing/src/beyond_sqlite/gate/cases.rs`. The skip list
does not remove a case from the run.

## Declared rejections

These scripts stay failures. Both engines exit non-zero and the target stderr
contains the listed text.

| Case | Text |
| --- | --- |
| 20021 | `invalid input value for enum color: "purple"` |
| 20023 | `value for domain positive_int2 violates check constraint "positive_int2_check"` |
| 20103 | `syntax error at or near "BY"` |
| 20111 | `SELECT DISTINCT ON expressions must match initial ORDER BY expressions` |
| 20136, 20247 | `cannot insert a non-DEFAULT value into column "id"` |
| 20203 | `NOT NULL constraint failed` |
| 20308 | `ERROR: boom` |
| 20340 | `extension "vector" is not available` |
| 20418, 20429 | `logical decoding requires wal_level >= logical` |
| 20438 | `syntax error at or near "ALL"` |

## Skip list

`metadata/beyond_sqlite/skip-list.toml` has 117 entries. All 117 are
`target_release = "closed"`. A closed entry stays in the file so the old
rationale remains, and it leaves the deferred denominator
(`skip_list.rs`: deferred means any value other than `closed`).

88 of those entries were still marked `deferred` after the corpus scripts had
started passing. That included `LISTEN`, `CREATE MATERIALIZED VIEW`, `citext`
equality, enum use, `INHERIT`, `search_path`, and a `BRIN` count. The gate
executed them. Closing the entry records that the **corpus script** passes.
It does not claim the PostgreSQL wire protocol, roles, or the rest of the
manual page.

The same file is copied at:

- `subrepos/redline-testing/metadata/beyond_sqlite/skip-list.toml` (the runner compiles this copy)
- `subrepos/redline/metadata/beyond_sqlite/skip-list.toml`

Those three copies stay identical.

## What this is not

PostgreSQL wire, TLS, roles, and SQLSTATE are unverified. `CREATE EXTENSION
vector`, logical decoding, `MERGE … WHEN NOT MATCHED BY SOURCE`, and
`LISTEN ALL` stay errors, as the table above says. SQLite format 3 is not the
runtime database.

## Historical triage

An earlier pass, against `target/redline-testing/beyond_pg_baseline.jsonl`,
triaged 233 failures: about 114 were written into this skip list and about
119 were called closable in pure Rust. The category sections that used to
live here ("14 skipped of 14 failed", and the same shape for procedures,
replication, materialized views, locks, collations, types, migration,
schemas, and indexes) described that triage. They are not the current score.
The current score is 265 / 265 with the 12 rejections above.
