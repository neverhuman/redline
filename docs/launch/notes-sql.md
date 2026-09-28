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

## U+E000 is an ordinary character outside Postgres citext (PG-03)

- `::citext` casts mark their value with a leading U+E000, and every text
  comparison treated that prefix as "compare without case" in every dialect.
  In the default SQLite dialect `char(57344)||'A' = 'a'` was true, a UNIQUE
  TEXT column rejected `char(57344)||'A'` next to `'a'`, DISTINCT collapsed
  values that `count(DISTINCT)` kept apart, and an indexed and a scanned
  `WHERE x = 'a'` returned different rows. The prefix now means "without
  case" only in a Postgres-dialect statement after `CREATE EXTENSION
  citext`; everywhere else U+E000 compares as the character it is, as in
  SQLite. A parallel sort carries the statement's dialect to its workers.
- The shell printed every TEXT value without a leading U+E000. It now hides
  the marker only in the Postgres dialect after citext was enabled.
- The Postgres dialect refuses a column declared `citext` (CREATE TABLE,
  ALTER TABLE ADD COLUMN, ALTER COLUMN TYPE) with
  `unsupported capability: citext column: ...`: such a column compared with
  case. Cast the values with `::citext` instead. The SQLite dialect keeps
  accepting `citext` as a declared type name (TEXT affinity).
- New error variant `Error::UnsupportedCapability { feature, detail }`
  (`unsupported capability: {feature}: {detail}`); the facade maps it to
  `ErrorCode::Unsupported`.

## TEXT operands follow SQLite unless an expression says Postgres (Q5-05)

- `||`, `-`, `+`, `*`, `/` and `%` read TEXT operands as jsonb, dates, exact
  decimals or trigram strings only when the dialect is Postgres or an
  operand is a Postgres value: a `::` cast to `numeric`, `decimal`, `date`,
  `timestamp`, `timestamptz`, `interval`, `json` or `jsonb`, a jsonb-returning
  function (`to_jsonb`, `jsonb_build_object`, `jsonb_set`, ...), or an
  operator over such an operand. Date subtraction needs a Postgres value on
  both sides. In the default SQLite dialect `'[1]'||'[2]'` is now `[1][2]`
  (was `[1, 2]`), `'2025-01-02'-'2025-01-01'` is 0 (was `1 day`),
  `'[1,2]'-0` is 0 (was `[2]`), `'7'%'4'` is 3 (was 0) and `'abc'%'abd'` is
  NULL (was 1). `0.1::numeric + 0.2::numeric`, `'[1]'::jsonb || '[2]'` and
  `'2025-01-02'::timestamp - '2025-01-01'::timestamp` keep their Postgres
  answers; the constant folder no longer folds those casts away.
- Arithmetic reads TEXT and BLOB operands the way SQLite does
  (`computeNumericType`): `'1'+2` is INTEGER 3 (was TEXT), `'7'/'2'` is 3
  (was 3.5), `'1.5'*2` is REAL 3.0, `'1abc'+1` is 2, `'1e2'+1` is 101.0 and
  `'abc'+1` is 1 (the last three failed with `datatype mismatch`); unary
  minus reads them the same way (`-'5'` is -5). The Postgres dialect still
  refuses non-numeric TEXT in arithmetic.
- Standard `CAST(x AS type)` in the SQLite dialect follows SQLite's type
  affinity rules: `DATE`, `TIMESTAMP`, `BOOLEAN`, `UUID`, `JSON` and `MONEY`
  are NUMERIC casts (`CAST('2025-01-02' AS DATE)` is INTEGER 2025), and
  `CAST(... AS NUMERIC)` follows `sqlite3VdbeMemNumerify` (`'1.0'` is
  INTEGER 1, `'inf'` and `'nan'` are 0). `::` casts and the Postgres dialect
  keep the Postgres readings.

## Comparison affinity (Phase 1 finding, datatype3.html §4.2)

- Comparisons now convert their operands by SQLite's comparison affinity
  (`sqlite3CompareAffinity`): a column has its declared affinity (a column
  without a type has BLOB affinity), `CAST(x AS T)` has T's, a scalar or
  `IN` subquery has its result column's, and literals, parameters and
  other expressions have none. If one operand is INTEGER, REAL or NUMERIC
  and the other is not, well-formed numeric TEXT is compared as its number;
  if one is TEXT and the other has no affinity, numbers are compared as
  text. This applies to `=`, `<>`, `<`, `<=`, `>`, `>=`,
  `IS [NOT] DISTINCT FROM`, `IN (list)` (left operand's affinity),
  `IN (SELECT ...)`, `BETWEEN` (each bound), `CASE x WHEN`, join
  conditions, UPDATE/DELETE WHERE, HAVING, trigger bodies and partial-index
  predicates. `WHERE x = '5'` on an INTEGER column now finds 5 (it found
  nothing), `WHERE y = 5` on a TEXT column finds '5', and a column without
  a type still does not match across types, as in SQLite.
- Index probes convert the constant by the indexed column's affinity, so an
  index and a scan give the same rows. A join probe uses the other table's
  index only when the comparison converts the probe the way the index does
  (`sqlite3IndexAffinityOk`); otherwise it compares row by row.
- `WHERE rowid = '5'` (and `id = '5'` on an INTEGER PRIMARY KEY, or a bound
  TEXT parameter) finds rowid 5. It used to fail with `comparing a rowid
  with numeric text needs comparison affinity`.
- As in SQLite, a trigger's `NEW.col`/`OLD.col` has no affinity (only the
  rowid is INTEGER), and `+x` has none.
- The constant folder keeps a `CAST` node around its folded value, so
  `CAST(5 AS INTEGER) = '5'` is 1 as in SQLite (it folded to `5 = '5'`,
  which is 0).
- Not yet: columns of views, CTEs and FROM-subqueries have no comparison
  affinity (SQLite gives them their defining expression's), so
  `WHERE x = '5'` over a view of an INTEGER column still finds nothing, as
  before. CHECK constraints are evaluated by the kernel without comparison
  affinity (`CHECK (x = '5')` rejects INTEGER 5, as before). The general
  `x IS y` form does not parse; `IS [NOT] DISTINCT FROM` does.
- Upgrade note: a partial index whose WHERE compares a column with a value
  of another type (`WHERE x = '5'`) was built without comparison affinity.
  A 4.x database rebuilds every index at its first open (index-format
  epoch 3), which rebuilds such an index with the new rule.
