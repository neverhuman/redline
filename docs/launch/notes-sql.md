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
- A frame offset near 2^63 answers at once. The sliding `sum()` / `total()`
  / `avg()` schedules looped once per row of the FOLLOWING offset, so
  `ROWS BETWEEN CURRENT ROW AND 9223372036854775807 FOLLOWING` spun for
  about 2^63 iterations, and `k + offset` overflowed (a panic in debug
  builds); every frame offset is now added with saturation.
- GROUPS frames count their offsets in peer groups: `GROUPS 1 PRECEDING`
  over peer rows reached back one row instead of one group, for every
  window function. Sliding GROUPS sums step, return and remove whole groups
  in SQLite's order, and a RANGE frame whose bounds are both PRECEDING or
  both FOLLOWING adds rows before it removes them, so `integer overflow`
  shows where SQLite reports it.
- Not yet: a RANGE frame with a numeric offset (`RANGE BETWEEN 1 PRECEDING
  AND CURRENT ROW`) still spans that many rows, not the ORDER BY values
  within the offset, so it answers like SQLite only when the ORDER BY values
  are consecutive integers.

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
  the column affinity, and read the heap for a column without affinity, for
  a STRICT table's ANY column (which keeps each value's class, so a stored
  `2.0` came back as INTEGER `2`) and for a NOCASE key (which returned the
  text lower-cased).
- The affinity now describes every stored value, which the covering scan
  relies on. Generated columns, STORED and VIRTUAL, take their declared
  affinity as in SQLite (`b REAL GENERATED ALWAYS AS (a)` over `a = 2` is
  REAL `2.0`, and a STRICT table checks their type on write); they kept the
  expression's class before. `ALTER TABLE ... ALTER COLUMN ... TYPE` converts
  the stored values to the new type, keeping indexes, generated columns and
  constraints in step; it used to change only the declared type, so after
  `TYPE REAL` a stored `3` stayed INTEGER. A value the new type refuses (a
  STRICT table) or two rows the conversion makes equal under a UNIQUE
  constraint fail the ALTER, which then changes nothing. Values stored by
  earlier builds are not rewritten until their row is.
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
- A comparison with an aggregate on one side (`HAVING g = count(*)`,
  `SELECT g, sum(v) = g ... GROUP BY g`, `sum(v) BETWEEN g AND g`,
  `g IN (count(*), ...)`) applies the same affinity: the aggregate has none
  and a GROUP BY column keeps its own, so over a TEXT column `'2' =
  count(*)` is true. The grouped evaluator compared the raw values and
  found nothing. `x NOT IN (NULL, x)` in a grouped expression is false, not
  NULL.
- Index probes convert the constant by the indexed column's affinity, so an
  index and a scan give the same rows. A join probe uses the other table's
  index when SQLite's `sqlite3IndexAffinityOk` allows it; a TEXT = TEXT
  equijoin (which converts nothing) was wrongly refused and compared every
  pair of rows.
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

## Unique-key locks for citext and NaN keys

- The SQL-side unique-key lock takes one lock for every spelling of a
  `::citext` text while citext comparisons are active (its unmarked ASCII
  lower-case form), so two writers inserting `'ABC'::citext` and
  `'abc'::citext` into the same UNIQUE key serialize instead of both passing
  the heap-scan conflict check. Every NaN payload takes one lock; the record
  encoder already folded NaN payloads and -0.0, and the lock key now does it
  explicitly.

## Compatibility rewrites leave literals, names and comments alone (Q5-01)

- The pre-parse rewrites that lower Postgres and SQLite surface syntax now
  change only SQL code. The same words inside a string literal, a quoted
  identifier or a comment are kept as written. `SELECT 'AS MATERIALIZED'`
  answered `AS` and INSERT stored the corrupted value; `NULL IS NOT 1`,
  `a window win as (b)`, `overriding system value`, `group by rollup (a)`,
  `create sequence ... start with`, `as identity (...)`, `drop identity`,
  `from pg_class`, `::regclass`, `array_length(`, `array_agg(`, `&&`,
  `interval '...'` and `at time zone` inside a literal were rewritten or broke
  the parse.
- `SELECT 1 AS materialized` and `SELECT c AS materialized FROM t` parse, and
  `SELECT 'a' AS materialized_col` keeps its column name. The CTE hint
  `AS [NOT] MATERIALIZED` is dropped only when a `(` follows it, including
  with a comment between the words.
- Non-ASCII text survives every rewrite. A statement that woke a byte-wise
  pass re-encoded each byte of a multi-byte character (`'café'` became
  `'cafÃ©'`). Any statement with `'-` or `'+` did this, so
  `INSERT INTO t VALUES('café', '-')` stored the corrupted text, and the GLOB
  pass ran on every statement, so an unquoted non-ASCII table name was
  renamed. `VACUUM INTO` with a non-ASCII path wrote to a mangled path.
- A multi-byte character next to `interval `, `at time zone`,
  `array_length(`, `array_agg(` or `GROUPING(` made a pass slice the SQL
  inside that character. Under `panic = "abort"` the release CLI aborted
  (`SELECT 'é interval '`).
- In the SQLite dialect `'\x41'` is the four-character text `\x41`, as in
  SQLite. Only the Postgres dialect reads it as a hex bytea literal.

## `pg_listening_channels()` (PG-02)

- The rewrite that answers `SELECT pg_listening_channels()` changes only a
  call in code. The call spelled inside `'...'`, `E'...'`, `$$...$$`,
  `$tag$...$tag$`, a quoted identifier or a comment is returned byte for
  byte; after `LISTEN`, a literal used to have the channel list spliced into
  it.
- A prepared `SELECT pg_listening_channels()` answers the channel set of the
  moment it runs, not of the moment it was prepared.
- Any other use of the function (`SELECT pg_listening_channels(), 1`,
  `SELECT upper(pg_listening_channels())`, `SELECT * FROM
  pg_listening_channels()`, an alias) fails with `unsupported capability:
  pg_listening_channels outside a bare SELECT list` instead of a parse error
  or a rewritten statement.

## Partial-index membership on UPDATE, `integrity_check` reads index contents (Q5-02)

- An UPDATE now keeps a partial index (`CREATE INDEX ... WHERE ...`) in step
  with the row. It never evaluated the index's WHERE clause: a row the UPDATE
  moved into the index (`flag` 0 to 1) got no entry, a row it moved out kept
  its entry, and a row outside the index whose key changed got one. Reads
  through the index missed rows (`SELECT id FROM t INDEXED BY ix_k WHERE
  flag = 1 AND k = 200` returned nothing, `count(*)` undercounted), and a
  partial UNIQUE index raised `UNIQUE constraint failed` for a key no member
  held or admitted a second member. Every UPDATE path (plain, hot-row, UPSERT
  DO UPDATE, REPLACE, MERGE, foreign-key cascades) goes through the fixed
  maintenance.
- `PRAGMA integrity_check` compares every index with its table: it derives
  the entries each row should have (evaluating expression keys and
  partial-index WHERE clauses) and reports `row N missing from index I`,
  `non-unique entry in UNIQUE index I`, `wrong # of entries in index I` and
  `index I has an entry for row N that the row does not produce`. It used to
  check only pages, the WAL and B-tree structure, and answered `ok` for the
  damaged indexes above. `PRAGMA quick_check` still skips index contents, as
  in SQLite.
- A row that leaves an index and comes back under the same key and rowid is
  in the index again. Its entry has the bytes of the tombstone it left, and
  the kernel insert took that tombstone for the entry: after `flag` 1 -> 0
  -> 1 the row was missing from its partial index, and a partial UNIQUE
  index admitted a second row with the key. Plain indexes lost the entry the
  same way on a key round trip (`k` 10 -> 11 -> 10) and on DELETE followed
  by an INSERT of the same rowid and key. The insert now restamps the
  tombstone when no snapshot can still read it (the same transaction removed
  it, or its insert rolled back), and otherwise keeps it for older snapshots
  and adds the entry's next version beside it.
- MERGE (`WHEN MATCHED THEN UPDATE` and `WHEN NOT MATCHED THEN INSERT`) and
  `ON UPDATE CASCADE` check UNIQUE constraints before they write, as UPDATE
  and INSERT do. They did not: a MERGE or a cascade could give a second row
  a key another row held, through a UNIQUE index, a partial UNIQUE index the
  row entered, or a column's own UNIQUE constraint. A cascade also recomputes
  the child's STORED generated columns now.
- A point lookup on a unique partial index no longer skips the routed table
  scan; a partial index holds only the rows its WHERE clause admits.
- An index with a `COLLATE NOCASE` (or RTRIM or custom) key is no longer
  probed: the probe used the raw constant against folded keys, so on a
  BINARY column `name = 'Gamma'` found nothing through
  `CREATE UNIQUE INDEX ... (name COLLATE NOCASE)`. Such queries scan.
- Not yet: the new check reports two index defects this change does not fix.
  `CREATE INDEX` on a table with rows stored before an `ALTER TABLE ... ADD
  COLUMN` reads those rows' keys from the wrong column (the rows lack the
  added column, and the kernel's backfill then skips no leading table id),
  so such an index misses rows; `REINDEX` repairs it. An index keyed on a
  VIRTUAL generated column gets NULL keys from both CREATE INDEX and DML, so
  lookups through it find nothing. A column's declared RTRIM collation is
  still not applied by `=` (scan or index), as before.

Upgrade note: the partial indexes of a 4.x database may hold the damage
above. Opening a 4.x database rebuilds every index from the heap
(index-format epoch 3, see above), partial indexes included, so they are
repaired automatically at the first open; the epoch did not need another
bump because no released build writes epoch 3. If the old UPDATE let two
rows share the key of a partial UNIQUE index, the open fails with `UNIQUE
constraint failed`, names the index, says so and changes nothing; delete
one of the rows with 4.x and open again. A database already at epoch 3
(built from this branch before the fix) is not rebuilt at open: run
`PRAGMA integrity_check`, and `REINDEX` any index it names.

## Parallel heap scans return one visible version per row (SCAN-VERSIONS)

- `Engine::parallel_scan_page_range` (and `PageBackedHeap`'s page-range
  scans) returned every tuple on a page whose own markers looked live. An
  UPDATE or DELETE leaves the old tuple on its page unchanged, so after an
  UPDATE the scan returned the old version of the row next to the new one,
  and a deleted row came back. The scan now starts from the row directory
  and reads each row as `get_for_relation` does: the version the snapshot
  sees (through the undo chain when the newest tuple is too new, rolled
  back or another transaction's), never a superseded or deleted one. Rows
  are placed by the page their newest tuple is on, so disjoint page ranges
  return disjoint rows.
- `Engine::heap_page_count` counted only the pages in the page file, so a
  scan bounded by it missed rows on pages still only in the buffer pool
  (every page before the first checkpoint). It now counts every allocated
  page (`BufferPool::allocated_page_count`).
- New `Engine::parallel_scan_relation(tx, rel_id, workers)` reads a whole
  relation for a transaction with no page range, so a concurrent UPDATE
  that moves a row to a new page cannot hide it. The SQL parallel
  covering-scan dispatch uses it. That dispatch is still not reachable from
  SQL: the covering scan takes only plans without aggregation, and the gate
  dispatches only plans with it.

## An outer LIMIT caps a recursive CTE only for row-by-row reads (Q5-03)

- A recursive CTE read by `SELECT ... FROM c LIMIT n [OFFSET m]` stopped
  recursing after n + m rows whatever the SELECT list was, so an aggregate,
  a window function or a subquery over the CTE saw only the first rows:
  over the five rows 1..5, `SELECT count(*) FROM c LIMIT 1` answered 1,
  `sum(x) OVER ()` 1, `sum(x), max(x), avg(x)` 1, 1, 1.0,
  `(SELECT count(*) FROM c)` 1, and a sibling CTE `d AS (SELECT count(*)
  FROM c)` 1. The recursion now stops early only when every SELECT-list
  item is `*`, `t.*` or an expression of column names, literals,
  parameters, operators, `CAST` and `COLLATE`, the query has no `WINDOW`
  or `QUALIFY`, and no other CTE of the `WITH` mentions the CTE.
- A function call (even a scalar one such as `abs(x)`) or a `CASE` in the
  SELECT list now also turns the pushdown off. An unbounded recursion
  (no terminating `WHERE`) read that way fails with `recursive CTE ...
  exceeded 10000 iterations` instead of answering from a truncated CTE;
  `SELECT x FROM c LIMIT 10` still stops after 10 rows.

## UNION, INTERSECT and EXCEPT compare rows as SQLite values (Q5-04)

- The compound set operations keyed each row by an unescaped string, so
  rows whose TEXT held the delimiter collided: `SELECT 'a|Tb', 'c' UNION
  SELECT 'a', 'b|Tc'` gave 1 row (SQLite 2), INTERSECT 1 (0), EXCEPT 0 (1).
  The key also kept storage classes apart, so INTEGER 1 and REAL 1.0 were
  two rows: `SELECT 1 UNION SELECT 1.0` gave 2 rows, INTERSECT none and
  EXCEPT one. Rows are now keyed by a tagged, length-prefixed encoding in
  which an integral REAL in the i64 range has its INTEGER's key (`1`,
  `1.0` and `-0.0` are one value; `9007199254740993` and
  `9007199254740992.0` stay two, as SQLite compares them exactly).
- The surviving row is SQLite's: `SELECT 1 UNION SELECT 1.0` answers REAL
  1.0 and `SELECT 1.0 UNION SELECT 1` INTEGER 1 (the right operand wins a
  tie), and INTERSECT and EXCEPT keep the left operand's row. Among equal
  rows inside one operand the first is kept, as the pinned SQLite 3.53.1
  does; SQLite 3.50 kept the last there.
- A recursive CTE's `UNION` deduplicated by stored-record bytes, so
  `WITH RECURSIVE r(x) AS (SELECT 1 UNION SELECT 1.0 FROM r)` had 2 rows;
  it now has one (INTEGER 1, the first seen, as in SQLite).
- Not yet: set operations ignore collations (`'a' COLLATE NOCASE UNION
  'A'` is still two rows).

## GROUP BY puts 1 and 1.0 in one group (NEW-02)

- GROUP BY keys were stored-record bytes, which keep the storage class, so
  `SELECT x FROM (SELECT 1 x UNION ALL SELECT 1.0) GROUP BY x` answered two
  groups (SQLite one), and a column holding both `2` and `2.0` split its
  count and sum across two rows. Group keys are now the Q5-04 equivalence
  key: INTEGER 1, REAL 1.0 and REAL -0.0 are one group, TEXT '1' is
  another, and 9007199254740993 and 9007199254740992.0 stay two. The group
  shows its first row, as SQLite does (`1.0, 1` shows REAL 1.0). This
  applies to the one-pass hash aggregate, the spilling hash aggregate, the
  morsel aggregator, window `PARTITION BY`, and `count(DISTINCT x)` /
  `sum(DISTINCT x)` (which counted 1 and 1.0 as two values).

## `ORDER BY <n>` sorts by the n-th result column (NEW-01)

- `ORDER BY 2` was rewritten to the second result column's name, and only
  when that name was `[A-Za-z0-9_]+`. Any other result column (`-x`,
  `x * 10`, `upper(g)`, an alias with a space) kept the integer as a
  constant outside the grouped, DISTINCT and window paths, so the rows
  came back unsorted: `SELECT -x FROM t ORDER BY 1` returned scan order. An aliased
  column (`SELECT x AS z FROM t ORDER BY 1`) failed with `no such column:
  z`, and `SELECT y AS x, x AS y FROM t ORDER BY 1` sorted by the source
  column `x`. A position now sorts by the result column itself in every
  path: plain, joined, grouped, one-pass grouped, DISTINCT, window,
  compound (`UNION ... ORDER BY 1`), VALUES and subquery wrappers.
- Positions follow SQLite's reading (`sqlite3ExprIsInteger`): `+1`, `(1)`,
  `- -1` and `1 COLLATE NOCASE` are positions; `1 + 0`, `1.0`, `'1'`, a
  bound parameter and a literal past 2147483647 are constants (`1 + 0`
  used to fold into a position). `ORDER BY 0`, `-1` or a position past the
  last column fails with `1st ORDER BY term out of range - should be
  between 1 and N` (0 and negative positions used to be ignored). A
  position sorts under the term's COLLATE, or else the result
  expression's (`SELECT g COLLATE NOCASE ... ORDER BY 1`).
- The grouped paths sort under an ORDER BY term's COLLATE
  (`GROUP BY g ORDER BY g COLLATE NOCASE` compared bytes).
- Not yet: `ORDER BY <alias>` on a query without GROUP BY, DISTINCT or a
  window function still fails with `no such column` (SQLite sorts by the
  aliased result column), and a HAVING clause cannot name an alias.
