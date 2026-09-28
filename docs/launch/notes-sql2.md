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
