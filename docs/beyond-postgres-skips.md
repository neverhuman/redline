# Beyond-Postgres record

The execution gate and this document now say the same thing.

The beyond-SQLite corpus is 265 cases, every one `ordered_rows`, compared with
PostgreSQL 16.15 through the SQL shell. A pass is normalized SQL-shell
transcript agreement: cells are separated by 0x1F and NULL prints as `NULL`
wrapped in 0x1E, so a `|` in a value or the text `'NULL'` cannot fake a match,
but the comparison is still text, not typed rows. On `d5d1c57bb` the regression file
`metadata/beyond_sqlite/postgres-regression.json` has an empty `failed_cases`
list, and every case agreed: 253 positive matches and 12 expected
rejections, 0 failed, 0 skipped. Hosted `parity (redline-testing-official)`
on that tree was green. Eleven of those matches came from stand-ins that did
not do the work: `pg_current_wal_lsn`, `pg_export_snapshot`, `pg_notify` and
`NOTIFY` answered with success-shaped values. PG-01 refuses them with
`unsupported capability:`, so a local gate now records 242 positive matches,
12 expected rejections and 11 declared unsupported, with 0 mismatches.

A pass is a *positive match* (both engines exit 0 and print the same
normalized rows) or an *expected rejection*: one of the 12 cases whose corpus
entry declares the error text the target must print (`expected_target_stderr_contains`).
A skip is not a pass. The gate in
`subrepos/redline-testing/src/beyond_sqlite/gate/cases.rs` does not trust the
runner's verdict: it re-derives each case's contract from the corpus and
re-checks every pass against the evidence in the raw record (both exits, equal
hashes of the complete normalized stdouts, the declared text in the recorded
target stderr, and a setup that ran on its own). A failure where PostgreSQL
succeeded and RedlineDB refused with `unsupported capability:` is counted as
*declared unsupported*, a failure ratcheted by the policy's
`declared_unsupported` list; every other failure is a *mismatch*, ratcheted by
`failed_cases`. The skip list does not remove a case from the run.

## Capability matrix

A case outcome says what one script printed. The capability matrix says what
the product supports. Its source is
[`metadata/beyond_sqlite/postgres-capabilities.json`](../metadata/beyond_sqlite/postgres-capabilities.json);
this table renders it, and `crates/sql/tests/beyond_sqlite_manifest.rs` fails
when the two disagree. `implemented` means the surface behaves as PostgreSQL
does; `partial` names the part that does not; `unsupported` covers absent,
rejected and stand-in behaviour; `unverified` means no lane exercises it.

| Capability | Status | Surface | Cases | Note |
| --- | --- | --- | --- | --- |
| `wire` | unsupported | PostgreSQL frontend/backend protocol (psql, libpq and PostgreSQL drivers) | — | redlinedb-server speaks its own RLDB framing, not the PostgreSQL protocol, so psql and PostgreSQL drivers cannot connect. The corpus drives the redlinedb CLI. |
| `tls` | unsupported | TLS for PostgreSQL clients (sslmode) | — | There is no PostgreSQL listener, so there is no PostgreSQL TLS. redlinedb-server has no TLS either. |
| `roles` | unsupported | Roles and authorization: CREATE ROLE, GRANT, REVOKE, ownership checks, row security | 20219, 20254 | There is one implicit role, redlinedb. current_user, OWNER TO CURRENT_USER and CREATE SCHEMA ... AUTHORIZATION CURRENT_USER report it (the cited cases); current_user, session_user, current_role and pg_get_userbyid exist only in the Postgres dialect. CREATE ROLE and GRANT fail as unsupported SQL, and no statement is authorization-checked. |
| `sqlstate` | unsupported | SQLSTATE codes and PostgreSQL error fields (DETAIL, HINT, position) | — | The shell prints RedlineDB error codes and English text; no SQLSTATE is produced. Expected rejections match on declared error text, not on SQLSTATE. |
| `notify_delivery` | unsupported | NOTIFY and pg_notify delivery to listening sessions | 20431, 20432, 20435, 20437, 20441, 20442, 20443, 20444 | NOTIFY is refused when it is prepared, and pg_notify when it is called, with unsupported capability: nothing delivers a notification, so the cited cases are declared unsupported (PG-01). Before, NOTIFY was a no-op and pg_notify returned without delivering anything, and both answered in the default SQLite dialect too. LISTEN and UNLISTEN keep a channel set per connection (cases 20430, 20433, 20434, 20439, 20440), and pg_notification_queue_usage() is 0 because nothing can be queued (case 20436). |
| `advisory_locks` | partial | pg_advisory_lock, pg_try_advisory_lock, pg_advisory_unlock, pg_advisory_unlock_all | 20410, 20411 | Session-level exclusive locks shared by the connections of one open database in one process and owned by the connection that took them (PG-01): pg_try_advisory_lock answers f while another connection holds the key, pg_advisory_lock waits up to the busy timeout and then fails with lock timeout, pg_advisory_unlock answers t only for a key this connection holds, locks nest, and pg_advisory_unlock_all or dropping the connection releases them. The bigint and (int, int) keys are separate key spaces; the void result prints as an empty cell. Not provided: the shared and transaction-level variants, locks between processes, and the WARNING PostgreSQL prints for an unheld key. Two-connection tests are in crates/sql/tests/pg_behavior_two_sessions.rs; the cited cases run in one session. |
| `transaction_ids` | partial | txid_current, pg_current_xact_id | 20406, 20407 | Both return the id of the statement's transaction (PG-01): the same value throughout BEGIN ... COMMIT, a different one in every transaction, including concurrent ones and each autocommit statement, and never reused. Differences from PostgreSQL: every transaction has an id from its start, read-only ones included; the ids are RedlineDB's own transaction counter, not 32-bit xids with an epoch; and pg_current_xact_id returns a bigint, not xid8. The cited cases check only that the value is a positive integer. |
| `wal_lsn` | unsupported | pg_current_wal_lsn, pg_wal_lsn_diff | 20416, 20417 | pg_current_wal_lsn is refused with unsupported capability: the WAL position is not exposed to SQL, so cases 20416 and 20417 are declared unsupported (PG-01). pg_wal_lsn_diff is arithmetic on two LSN literals: the signed byte distance ('0/10' minus '0/0' is 16, the reverse -16), invalid input syntax for type pg_lsn for a malformed LSN, and unsupported capability for a distance outside the bigint range. With no LSN source, that arithmetic is all it offers. |
| `snapshot_export` | unsupported | pg_export_snapshot and SET TRANSACTION SNAPSHOT | 20423 | pg_export_snapshot is refused with unsupported capability: no other session can import a snapshot, so case 20423 is declared unsupported (PG-01). SET TRANSACTION SNAPSHOT is not supported. |
| `logical_replication` | unsupported | Logical decoding, replication slots, publications and subscriptions (CDC) | 20418, 20419, 20420, 20421, 20424, 20425, 20426, 20427, 20428, 20429 | Cases 20418 and 20429 are expected rejections: both engines refuse logical decoding while wal_level is replica. The other cited cases count empty catalog shims or accept DDL that replicates nothing. |
| `vector` | unsupported | The pgvector extension | 20340 | Case 20340 is an expected rejection: CREATE EXTENSION vector fails with extension "vector" is not available on both engines. |
| `citext` | partial | The citext extension | 20038, 20039, 20040 | Values cast with ::citext compare and order without case in =, <, ORDER BY and DISTINCT (the cited cases). A column declared citext is refused with unsupported capability: citext column, because it would compare with case. The cast prefixes the value with U+E000, which length, octet_length and substr count ('Hello'::citext has length 6). count(DISTINCT), GROUP BY and indexes still compare cast values by their bytes. The prefix means "without case" only in Postgres-dialect statements after CREATE EXTENSION citext; everywhere else, including every SQLite-dialect connection, U+E000 is an ordinary character (PG-03). |
| `multi_session` | unverified | Behaviour across sessions: locks, notifications, snapshots and catalog visibility between connections | — | Every corpus case runs in one fresh process on :memory:, so no corpus case observes a second session, and no lane compares two sessions with PostgreSQL. Native two-connection tests (crates/sql/tests/pg_behavior_two_sessions.rs, PG-08) check that advisory locks contend and are released when a connection drops, LISTEN channel sets are per connection, NOTIFY is refused, concurrent transactions have distinct txid_current values, tables, indexes and views one connection creates are in the other's pg_class, and a refreshed materialized view is refreshed for both. Known gap, pinned by that file: sequences and the pg_matviews catalog are kept per connection, so a second connection cannot call nextval on a sequence the first one created. |

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

`metadata/beyond_sqlite/skip-list.toml` has 117 entries. 106 are
`target_release = "closed"`, and the 11 declared-unsupported cases are
`target_release = "unsupported"`. A closed entry stays in the file so the old
rationale remains, and it leaves the deferred denominator
(`skip_list.rs`: deferred means any value other than `closed`). The runner
refuses a skip list in which any entry, closed or not, names a case id the
corpus lacks or a name other than the manifest's.

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

PostgreSQL wire, TLS, roles and SQLSTATE are unsupported, and behaviour
across sessions is unverified; the capability matrix above has a row for
each. `CREATE EXTENSION vector`, logical decoding, `MERGE … WHEN NOT MATCHED
BY SOURCE`, and `LISTEN ALL` stay errors, as the declared-rejection table
says. SQLite format 3 is not the runtime database.

## Historical triage

An earlier pass, against `target/redline-testing/beyond_pg_baseline.jsonl`,
triaged 233 failures: about 114 were written into this skip list and about
119 were called closable in pure Rust. The category sections that used to
live here ("14 skipped of 14 failed", and the same shape for procedures,
replication, materialized views, locks, collations, types, migration,
schemas, and indexes) described that triage. They are not the current score.
The current score is 265 / 265 with the 12 rejections above.
