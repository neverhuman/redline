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
with the old truthiness. Run `REINDEX` on such indexes after upgrading.

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
