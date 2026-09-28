# Security-relevant behaviour of the C ABI

This page is for programs that use SQLite's hooks and flags to make security
decisions and link against `libredlinedb` through the `sqlite3_*` names. It
lists what RedlineDB enforces, what it refuses so that a caller does not get a
false `SQLITE_OK`, what it accepts but does not act on, and what it does not do
yet. When this page and upstream SQLite disagree, this page describes
RedlineDB.

The rule for the refusals is fail closed: when RedlineDB cannot provide a
restriction a caller asks for, the call fails instead of succeeding and doing
nothing.

## Enforced

- **Registrations belong to their connection.** Functions
  (`sqlite3_create_function`, `_v2`, `16`, `sqlite3_create_window_function`)
  and collations (`sqlite3_create_collation`, `_v2`) are visible only on the
  connection that registered them. Closing the connection removes them, so a
  connection opened later at the same handle address starts with none. The
  `sqlite3_collation_needed` callback is also per connection.
- **Destructors run once.** For functions, once `db` is valid the call owns
  `user_data`: `destroy(user_data)` runs exactly once, when the call fails,
  when the function is replaced, when it is deleted (every callback NULL, which
  returns `SQLITE_OK`), or when the connection closes. For collations it runs on
  replace, delete (`compare` NULL) and close; as upstream documents, a failed
  `sqlite3_create_collation_v2` does not call it, and the caller keeps
  `user_data`. A destructor never runs while a registry lock is held, and never
  while another thread is still inside a callback using that `user_data`.
- **Authorizer decisions.** `sqlite3_set_authorizer` is consulted when a
  statement steps, and for a CTE body when the statement is prepared (the
  engine reads a CTE body while it prepares the statement, so
  `sqlite3_prepare*` itself fails with `SQLITE_AUTH` there). This holds for
  `sqlite3_exec` and for `sqlite3_prepare*` with `sqlite3_step`:
  - `SQLITE_SELECT` (21) once for each base table a SELECT reads, including
    tables reached through views, subqueries, CTEs and compound SELECTs, with
    the table name in `arg3`. Upstream passes NULL there and asks `SQLITE_READ`
    per column instead (see "Not implemented yet").
  - `SQLITE_INSERT` (18) and `SQLITE_DELETE` (9) for the target table of each
    DML statement, including statements in trigger bodies, with the table in
    `arg3` and NULL in `arg4`.
  - `SQLITE_UPDATE` (23) once for each assigned column, in `SET` order, with
    the table in `arg3` and the column name in `arg4`, as upstream does.
  - `arg5` is the database name, `"main"`. `arg6` is always NULL.

  `SQLITE_DENY` fails the statement with `SQLITE_AUTH` ("not authorized").
  `SQLITE_IGNORE` makes a SELECT return no rows and makes INSERT or DELETE
  change nothing. For `SQLITE_UPDATE` it leaves that one column unchanged
  while the other assigned columns update, as upstream does; when every
  assigned column is ignored the UPDATE changes nothing and reports no
  changed rows (upstream still counts the rows it visited). Any other
  return value fails the statement with `SQLITE_ERROR` and the message
  "authorizer malfunction", as upstream does; before v5 it was treated as
  `SQLITE_OK`.
- **Read-only blob handles.** `sqlite3_blob_open` with `flags == 0` opens a
  read-only handle. `sqlite3_blob_write` on it fails with `SQLITE_READONLY`
  and changes nothing.

Proof: `cargo test -p redlinedb-ffi --locked --test udf_register --test
collation_register --test hooks --test authorizer_paths --test blob_io`.

## Refused

These calls return an error, change nothing, and (for function registrations)
release `user_data` through its destructor.

| Call | Result | Why |
| --- | --- | --- |
| `sqlite3_create_function*` with `SQLITE_DIRECTONLY` | `SQLITE_ERROR`, "SQLITE_DIRECTONLY is not enforced by RedlineDB" | RedlineDB does not track whether a call comes from a view, a trigger or a schema expression, so it cannot keep a function out of them. |
| `sqlite3_create_function*` with a flag bit other than the four in "Accepted, not acted on" (for example `SQLITE_SELFORDER1`), or a text encoding of 6 or 7 | `SQLITE_ERROR`, "unsupported sqlite3_create_function flags" | An unknown flag may ask for a restriction RedlineDB does not provide. |
| `sqlite3_create_window_function` with a non-NULL `xValue` or `xInverse` | `SQLITE_ERROR` | The executor has no path that calls them. With both NULL the call registers an aggregate. |
| `sqlite3_trace_v2` with a callback and a non-zero mask | `SQLITE_ERROR`, "sqlite3_trace_v2 events are not supported by RedlineDB" | No `SQLITE_TRACE_*` event is ever delivered. A NULL callback or a zero mask returns `SQLITE_OK`, turns tracing off, and cancels a `sqlite3_trace` callback. |
| `sqlite3_prepare_v3` with a flag other than `PERSISTENT` or `NORMALIZE` | `SQLITE_ERROR`, "unsupported sqlite3_prepare_v3 flags", NULL statement | See `docs/exceptions/ffi-c-header.md`. |

## Accepted, not acted on

- `SQLITE_DETERMINISTIC`, `SQLITE_INNOCUOUS`, `SQLITE_SUBTYPE` and
  `SQLITE_RESULT_SUBTYPE` are accepted and recorded with the function. They
  restrict nothing: RedlineDB does not refuse non-deterministic functions in
  index expressions, CHECK constraints or generated columns, has no
  `trusted_schema` setting, and does not carry subtypes
  (`sqlite3_result_subtype` does nothing and `sqlite3_value_subtype` returns 0).
- The text encoding of a function registration (0 through 5) is accepted.
- The `database` argument of `sqlite3_blob_open` is not read; blobs are always
  opened in `main`.

## Not implemented yet

An application must not rely on any of these for access control.

- The authorizer is never asked `SQLITE_READ` (per column). An authorizer
  written for SQLite that denies `SQLITE_READ` on a secret table or column is
  never consulted for it; deny `SQLITE_SELECT` on the table instead.
- The authorizer is never asked about DDL (`CREATE`, `DROP`, `ALTER`),
  `PRAGMA`, `ATTACH`/`DETACH`, transactions and savepoints, function calls,
  `ANALYZE`, `REINDEX` or recursive CTEs. Apart from CTE bodies it is
  consulted when a statement steps, not when it is prepared as upstream does,
  so an authorizer set between prepare and step still applies to the step.
- `SQLITE_DIRECTONLY` is refused rather than enforced; enforcing it needs the
  call's origin (view, trigger, schema expression) carried through view and
  trigger expansion.
- `sqlite3_trace_v2` events (`STMT`, `PROFILE`, `ROW`, `CLOSE`) are never
  produced.
- `sqlite3_trace`, `sqlite3_profile` and the commit and rollback hooks fire
  only from `sqlite3_exec`/`rldb_exec`, not from statements run with
  `sqlite3_prepare*` and `sqlite3_step`. The commit hook fires only after an
  explicit `COMMIT` or `END`, and its non-zero return does not roll anything
  back.
- `sqlite3_busy_handler` stores its callback but RedlineDB never calls it.
- `sqlite3_collation_needed` stores its callback but RedlineDB never calls it;
  a statement naming an unknown collation fails with "no such collation
  sequence: NAME".
