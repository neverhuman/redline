# SQL lane release notes (v5.0.0 launch)

Release-note lines from the SQL correctness lane. The integrator folds them
into `CHANGELOG.md`.

## Integer arithmetic, comparison and truth values (Q5-06)

- INTEGER `+`, `-`, `*` and unary minus that overflow i64 now answer REAL, as
  SQLite does. They used to wrap (`9223372036854775807 + 1` gave
  `-9223372036854775808`), and debug builds panicked. `MIN / -1` answers REAL
  9.2233720368547758e+18 and `MIN % -1` answers 0 instead of NULL.
  `abs(-9223372036854775808)` raises `integer overflow`. Under the Postgres
  result dialect an INTEGER overflow is the error `bigint out of range`.
- `%` with a REAL operand casts both operands to INTEGER first, as SQLite does
  (`5.5 % 2` is `1.0`, `7 % 0.5` is NULL). A REAL result that is NaN is NULL.
- `UPDATE t SET x = x + 1` at `i64::MAX` stores REAL 9.2233720368547758e+18
  instead of a wrapped INTEGER.
- INTEGER and REAL values compare exactly (`sqlite3IntFloatCompare`):
  `9007199254740993 = 9007199254740992.0` is now false and `>` is true. This
  applies to `=`, `<`, `>`, ORDER BY, `min()` and `max()`, and to CHECK
  constraints evaluated by the kernel.
- TEXT and BLOB truth values use SQLite's numeric prefix: `'1abc'`, `x'31ff'`,
  `'1e'` and `'.5x'` are true; `'abc'`, `'inf'` and `'nan'` are false. This
  applies to WHERE, CASE WHEN, NOT, AND, OR, trigger WHEN, partial-index WHERE
  and CHECK.
- The CLI no longer aborts with a core dump on
  `SELECT (-9223372036854775807-1)/-1` or `% -1`; the ShellZero fast path
  hands those to the engine.

Upgrade note: a partial index whose WHERE clause reads a TEXT or BLOB value
as a truth value (for example `WHERE flag` with `flag = '1abc'`) was built
with the old truthiness. Opening a 4.x database rebuilds every index (see the
index-format epoch below), so such an index is rebuilt at the first open.

## `sum()` overflow on every route (S9-06)

- `sum()` raises `integer overflow` when its all-INTEGER running total leaves
  the i64 range, on every execution path: plain aggregates, the one-pass
  GROUP BY hash aggregate (which used to saturate), DISTINCT, FILTER, window
  functions (which used to answer REAL) and the morsel aggregator. A REAL
  input keeps the sum approximate, as in SQLite.
- `sum()`, `total()` and `avg()` share one accumulator that mirrors SQLite's
  `sumStep`: exact INTEGER sums past 2^53, Kahan-Babuska-Neumaier compensated
  REAL sums, and TEXT/BLOB inputs read as numbers the way SQLite reads them.
- Window `sum()`, `total()` and `avg()` over a sliding frame follow SQLite's
  add/remove accumulator, so a REAL or an overflow that passed through the
  frame shows in later rows as it does in SQLite.
- A window frame that ends before the partition starts (`ROWS BETWEEN 1
  PRECEDING AND 1 PRECEDING` on the first row) is empty; it used to read
  row 0.

## One numeric index key space, index-format epoch 3, and REINDEX (IDX-EPOCH)

- Index keys put INTEGER and REAL in one key space ordered by numeric value,
  the way SQLite compares them. Through an index, `x < 2` no longer loses a
  stored `1.5`, `x = 2` finds a stored `2.0`, `ORDER BY x LIMIT 3` no longer
  returns every INTEGER before any REAL, and a UNIQUE index rejects `1.0`
  when `1` is present (4.x accepted both in a column without affinity).
- The index-format epoch is the `u16` at offset 4 of every B-tree page:
  2 in 4.x, 3 now. Opening a database written by RedlineDB 4.x rebuilds every
  index from the heap, expression and partial indexes included, in one
  transaction, before the database is handed out. A rebuilt index gets a new
  B-tree, so the 4.x B-tree's WAL records are never replayed into it. If a
  UNIQUE index now holds two rows with one key (such as `1` and `1.0`), the
  open fails with `UNIQUE constraint failed: ...` naming the index and
  changes nothing; delete the duplicates with 4.x and open again.
- Downgrade: RedlineDB 4.x refuses a database that has any index at epoch 3
  instead of misreading its keys. Checked with the v4.1.0 release binary
  (`target/version-history/v4.1.0/redlinedb`, source af2082631): it prints
  `Error: 1: kernel error: unsupported format version: 3` and exits 1. The
  refusal is 4.x's own gate in `rehydrate_index_handles`
  (`crates/kernel/src/engine/catalog_ops/index.rs`, identical since v4.0.3):
  epoch 2 opens, epoch 1 is rebuilt, anything else fails `Engine::open`. A
  database with no index has no epoch to check and still opens in 4.x. This
  build refuses an index from a newer epoch the same way.
- `REINDEX` rebuilds indexes; it was a no-op, and only the bare statement
  parsed. `REINDEX`, `REINDEX table`, `REINDEX index`, `REINDEX main.name`,
  `REINDEX temp.name` and `REINDEX collation` resolve names as SQLite does
  (collation first, then table, then index, otherwise `unable to identify
  the object to be reindexed`). A rebuild takes effect at COMMIT. REINDEX of
  an attached database is not supported yet, and a custom collation is
  recognized only when some index uses it.
- A leading DESC index key took its value bounds in ascending byte order, so
  `WHERE x > 1` through a DESC index returned no rows, and an ordered LIMIT
  walked a DESC key in the wrong direction. An index range with no lower
  bound started at the NULL keys, so `WHERE x < 5 ORDER BY x LIMIT 1` spent
  its row on a NULL and returned nothing.
- Index-only (covering) scans take the storage class of a whole number from
  the column affinity, and read the heap for a column without affinity and
  for a NOCASE key (which returned the text lower-cased).
- `PRAGMA redline_full_check` picks the record layout that matches the most
  index entries; it used to take the first layout that matched any entry and
  then report a neighbouring column's values as mismatches.
- A rebuild leaves the old B-tree's pages allocated, as `DROP INDEX` does.

## The dialect is a per-database option (PG-09)

- `DbOptions::dialect` (`redlinedb_sql::Dialect::{Sqlite, PostgresSubset}`)
  and `OpenOptions::with_dialect` (`redlinedb`) choose a database's SQL
  dialect. It is fixed when the database opens and every connection to it
  speaks it, so a Postgres-dialect database and a SQLite-dialect database
  can be used side by side in one process.
- The engine no longer reads `REDLINEDB_RESULT_DIALECT` while it evaluates a
  statement (it used to read it on every boolean result, comparison and
  truth test). When the option is `None`, the variable is read once, as the
  database opens; changing it afterwards has no effect on an open database.
  The shell reads it once at startup and passes it to every database it
  opens, so `REDLINEDB_RESULT_DIALECT=postgres redlinedb` behaves as before.
- Prepared-statement cache keys include the dialect. An attached database
  takes the dialect of the statement that attaches it. Opening a path that
  is already live in the process under the other dialect fails with
  `database already open with incompatible options`.
