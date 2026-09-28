# SQL lane 2 release notes (v5.0.0 launch)

Release-note lines from the second SQL correctness lane. The integrator folds
them into `CHANGELOG.md`.

## Postgres session functions do the work or refuse (PG-01, PG-08)

- The Postgres session functions exist only in the Postgres dialect. A
  SQLite-dialect database used to answer `SELECT pg_advisory_unlock(1),
  pg_wal_lsn_diff('0/10','0/0'), pg_notify('a','b'), txid_current()` with
  `1|1|1|1`; it now reports `unsupported function`, as sqlite3 reports
  `no such function`. The same holds for `repeat`, `pg_backend_pid`,
  `current_user`, `session_user`, `current_role` and `pg_get_userbyid`.
- Advisory locks are real session-level locks between the connections of one
  open database: `pg_try_advisory_lock` answers `f` while another connection
  holds the key, `pg_advisory_lock` waits up to the busy timeout and then
  fails with `lock timeout`, `pg_advisory_unlock` answers `t` only for a key
  the connection holds (it used to answer `t` for any key), locks nest, and
  `pg_advisory_unlock_all()` or dropping the connection releases them. The
  `void` result prints as an empty cell, as psql prints it.
- `txid_current()` and `pg_current_xact_id()` return the id of the
  statement's transaction instead of the constant 1.
- `pg_wal_lsn_diff(a, b)` returns the signed byte distance (`'0/10'` and
  `'0/0'` give 16 and -16; it used to answer 1 for any two different LSNs)
  and rejects a malformed LSN with `invalid input syntax for type pg_lsn`.
- `pg_current_wal_lsn`, `pg_export_snapshot`, `pg_notify` and the `NOTIFY`
  statement fail with `unsupported capability:` instead of answering with a
  constant or doing nothing. `NOTIFY` is refused when it is prepared.
- The advisory-lock and transaction-id functions refuse to run while a
  statement is prepared (a VALUES list, CTE, derived table or view evaluated
  at prepare), where they would act once instead of per execution.
- Postgres corpus: 254/265 agree (242 row matches and 12 expected
  rejections); the 11 cases that call the refused functions are declared
  unsupported in `metadata/beyond_sqlite/postgres-regression.json`.

## Savepoint statements act when stepped (S9-04)

- `SAVEPOINT`, `RELEASE` and `ROLLBACK TO` take effect when their statement
  is stepped, as in SQLite, not when it is prepared. Preparing `RELEASE s`
  used to commit the transaction that `SAVEPOINT s` had opened without a
  step, preparing `ROLLBACK TO s` discarded work, and reset + step of a
  prepared savepoint statement did nothing. An unknown savepoint name is now
  reported by the step. `sqlite3_stmt_readonly` is true for these statements.

## ROLLBACK TO keeps the snapshot and refuses what it cannot replay (S9-05)

- `ROLLBACK TO` re-executes the statements before the savepoint in the
  transaction's own snapshot instead of a fresh one, so rows another
  connection committed meanwhile no longer appear after it, and it keeps a
  `BEGIN IMMEDIATE` / `BEGIN EXCLUSIVE` reservation instead of dropping it.
- It refuses, before changing anything, when one of those statements is not
  replay-safe: it read the clock, drew a random value, called a user-defined
  or Postgres function, read `changes()`, `last_insert_rowid()`, a sequence
  or the transaction id, fired a trigger, used `RETURNING`, or changed the
  schema. The transaction is then failed and only `ROLLBACK` ends it. These
  statements used to be re-evaluated, storing different values than the
  ones the transaction had seen.
- `RELEASE` of the last savepoint inside `BEGIN` no longer forgets the
  statements before it: `BEGIN; SAVEPOINT a; INSERT ...; RELEASE a;
  SAVEPOINT b; ...; ROLLBACK TO b` used to lose the first insert.

## CTE, derived-table and view rows belong to their statement (Q5-09)

- A nested `WITH` no longer overwrites the outer CTE's rows:
  `WITH a(x) AS (SELECT 11) SELECT a.x, s.y FROM a JOIN (WITH b(y) AS
  (SELECT 22) SELECT y FROM b) s ON 1` answers `11|22` (it answered
  `22|22`), also through a view whose body has a `WITH`.
- A CTE name no longer shadows a real table in later statements on the same
  thread. After `WITH t(a) AS (SELECT 99) SELECT a FROM t`, a plain
  `SELECT a FROM t` read 99, `INSERT INTO t SELECT a+1 FROM t` stored 100
  and `DELETE FROM t WHERE a IN (SELECT a FROM t)` deleted nothing; the same
  leak crossed connections and databases on one thread, and a CTE whose
  binding failed left its scope behind.
- Two prepared statements with CTE joins no longer read each other's rows.
- A view body and a trigger body resolve names in their own scope: a CTE in
  the statement that uses the view or fires the trigger no longer replaces
  the table the body names.
- The rows a statement's binding materializes are freed with the statement,
  and a template that embeds them is never served from a statement cache
  (an `INSERT ... SELECT` joining a derived table or view used to be cached
  with the rows of its first preparation).
