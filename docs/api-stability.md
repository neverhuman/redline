# API stability

What each RedlineDB interface promises across releases, starting with v5.0.0.
Where this page and a release note disagree, the release note describes that
release and this page is out of date; report it.

| Interface | Status | What a release may change |
| --- | --- | --- |
| Rust crates (`redlinedb` and the rest of the workspace) | Unstable, not on crates.io | Anything, in any release |
| C API: `rldb_*`, `redlinedb.h` | Stable within ABI major 5 | Additions only; an incompatible change raises the major |
| C API: the `sqlite3_*` subset | Experimental | Behaviour, per symbol, in a minor release; no removal within major 5 |
| Database files | Native format, versioned per structure | A newer release may upgrade a database so older releases refuse it |
| `redlinedb` command line | Stable option names within 5.x | Output details; maintenance subcommands are experimental |
| Panics | Fail-stop: the process aborts | Only by a recorded decision |

## Rust crates: unstable, not published

No workspace package is published to crates.io. `[workspace.package]` in
`Cargo.toml` sets `publish = false` and every member inherits it with
`publish.workspace = true`, so `cargo publish` refuses them.
`scripts/check-publish-policy.sh`, run by the CI preflight
(`ops/ci/fast.sh`), fails if a package loses that setting. The names
`redlinedb` and `redlinedb-ffi` on crates.io do not belong to this project.

Depend on a release tag and commit your `Cargo.lock`:

```toml
[dependencies]
redlinedb = { git = "https://github.com/neverhuman/redline", tag = "v5.1.1" }
```

The Rust API of the `redlinedb` facade has no semver promise: any release,
including a patch release, may rename, remove or change a public item. Read
the release notes before you move the tag. The other crates (`redlinedb-kernel`,
`redlinedb-sql`, `redlinedb-domain`, `redlinedb-bench`, `redlinedb-tokio`,
`redlinedb-sqlx`, `redlinedb-server`, `redlinedb-lite`, `redlinedb-cli`) are
internal to the workspace. The `rlib` that `redlinedb-ffi` builds exists for
that crate's own tests and is not a Rust API (see
`docs/compatibility/abi-safety.md`).

## C API: `rldb_*` is stable within ABI major 5

The library carries its ABI major in its name: `libredlinedb.so.5` (ELF
soname) and `@rpath/libredlinedb.5.dylib` (Mach-O install name).
`RLDB_ABI_MAJOR` in `contracts/c-abi/redlinedb.h` is the single source of that
number. The stable surface is exactly the `rldb_*` functions, the `rldb_*`
types and the `RLDB_*` constants declared in `redlinedb.h`;
`crates/ffi/tests/api_stability.rs` fails if an exported `rldb_*` function is
not declared there or a declared one is not exported.

Within major 5:

- A declared function is not removed, and its signature and calling
  convention do not change. The value of a declared constant does not change.
- `rldb_config` grows only at its end. `struct_size` versions it: a program
  built against an older, shorter struct keeps working, and a field it does
  not set keeps its default. A newer program that sets a field an older
  library does not know gets `RLDB_MISUSE` instead of having the field
  ignored. The rules are next to `rldb_config` in `redlinedb.h`.
- New functions and constants may be added in a minor release. A program
  built against 5.y runs against any later 5.z library, not against an
  earlier one.
- Behaviour changes only to fix a bug against the documented contract (the
  header, `docs/compatibility/abi-safety.md`,
  `docs/exceptions/ffi-c-header.md`). Such a fix can make input that used to
  be accepted fail closed; it is listed in the release notes.
- Result codes are part of the contract; `rldb_errmsg` text is not.

A change that breaks any of these raises `RLDB_ABI_MAJOR`, and with it the
library name, in the same change, as `docs/exceptions/ffi-c-header.md`
requires. v5 has no compatibility alias for v4 programs; they must be
rebuilt.

## C API: the `sqlite3_*` subset is experimental

The same library exports a subset of SQLite's C API so that some SQLite
programs can link against it. It is not a drop-in `libsqlite3`:

- Many upstream functions are not exported, among them `sqlite3_bind_int`,
  `sqlite3_column_int`, `sqlite3_bind_parameter_count`, `sqlite3_prepare`
  and every UTF-16 variant. A program that calls one fails to link.
  `crates/ffi/tests/symbol_allowlist.toml` lists each missing symbol with its
  reason.
- The database behind the API is RedlineDB's own format, not a SQLite file
  (see "Database files").
- The shipped `sqlite3.h` is a shim that includes `redlinedb.h`; it declares
  only part of the subset. The symbols marked "no" under "Header" below are
  exported but declared only in upstream's header, whose values RedlineDB
  matches.

Within major 5 no exported `sqlite3_*` symbol is removed and no signature
changes. Behaviour may change in a minor release, to move closer to upstream
or to fail closed where RedlineDB cannot do what upstream does; each change is
in the release notes. `docs/security-capabilities.md` lists the hooks and
flags that are enforced, refused or inert.

Statuses in the matrix:

- **tested**: follows upstream for the cases the listed tests cover (the
  tests call the symbol or the function it forwards to, marked "rldb" when
  that is the `rldb_*` function).
- **partial**: works, with the difference given in the notes.
- **refuses**: some input upstream accepts fails with an error; otherwise as
  tested.
- **inert**: accepted, but does nothing or returns a fixed value.
- **untested**: written to follow upstream, but no test here calls it.
- **redlinedb-only**: upstream has no function of this name.

Tests are under `crates/ffi/tests/` unless noted: `src/tests` is
`crates/ffi/src/tests.rs`, `storage_class` is
`crates/ffi/src/tests_storage_class.rs`, and `probe` is
`contracts/c-abi/probe/phase2_abi_probe.c`, run by
`scripts/compatibility/phase2-abi-probe.sh` against upstream's header.
`crates/ffi/tests/api_stability.rs` fails if this table misses an exported
`sqlite3_*` symbol, lists one that is not exported, or gets the Header column
wrong.

| Symbol | Header | Status | Notes | Tests |
| --- | --- | --- | --- | --- |
| `sqlite3_aggregate_context` | no | untested | | |
| `sqlite3_backup_finish` | no | partial | See `sqlite3_backup_step`. | safety_invariants (rldb) |
| `sqlite3_backup_init` | no | partial | The destination is the other connection's database path; the schema-name arguments are not read. An in-memory destination fails with `SQLITE_CANTOPEN`. | open_memory |
| `sqlite3_backup_pagecount` | no | partial | A nominal count (1), not pages. | safety_invariants (rldb) |
| `sqlite3_backup_remaining` | no | partial | 1 before the copy and 0 after it, not pages. | safety_invariants (rldb) |
| `sqlite3_backup_step` | no | partial | The first step copies the whole database directory, whatever the page count asked for. | safety_invariants (rldb) |
| `sqlite3_bind_blob` | yes | partial | The destructor is never called; the bytes are copied. A negative `nbytes` panics, which aborts a release build. | safety_invariants, exec_input_boundary (rldb) |
| `sqlite3_bind_double` | yes | tested | | safety_invariants (rldb) |
| `sqlite3_bind_int64` | yes | tested | | src/tests |
| `sqlite3_bind_null` | yes | tested | | safety_invariants (rldb) |
| `sqlite3_bind_parameter_index` | yes | tested | | error_paths, safety_invariants (rldb) |
| `sqlite3_bind_text` | yes | partial | The destructor is never called; the bytes are copied. | src/tests |
| `sqlite3_bind_value` | no | untested | | |
| `sqlite3_bind_zeroblob` | no | untested | | |
| `sqlite3_bind_zeroblob64` | no | untested | | |
| `sqlite3_blob_bytes` | yes | tested | | blob_io |
| `sqlite3_blob_close` | yes | tested | | blob_io |
| `sqlite3_blob_open` | yes | partial | The database-name argument is not read; blobs open in `main`. `flags == 0` gives a read-only handle. | blob_io |
| `sqlite3_blob_read` | yes | tested | | blob_io |
| `sqlite3_blob_reopen` | yes | tested | | blob_io |
| `sqlite3_blob_write` | yes | tested | Returns `SQLITE_READONLY` on a read-only handle. | blob_io |
| `sqlite3_busy_handler` | yes | inert | The callback is stored and never called. | hooks |
| `sqlite3_busy_timeout` | yes | tested | | src/tests |
| `sqlite3_changes` | yes | tested | | src/tests |
| `sqlite3_changes64` | yes | untested | | |
| `sqlite3_checkpoint` | yes | redlinedb-only | Checkpoints the native WAL (upstream has `sqlite3_wal_checkpoint`, not exported). | safety_invariants (rldb) |
| `sqlite3_clear_bindings` | yes | tested | | safety_invariants (rldb) |
| `sqlite3_close` | yes | tested | `SQLITE_BUSY` while statements are unfinalized, as upstream. | open_memory, upstream_abi, probe |
| `sqlite3_close_v2` | yes | partial | Same as `sqlite3_close`: returns `SQLITE_BUSY` with unfinalized statements instead of deferring the close. | error_paths (rldb) |
| `sqlite3_collation_needed` | yes | inert | The callback is stored per connection and never called; an unknown collation fails the statement. | collation_register |
| `sqlite3_column_blob` | yes | tested | | exec_input_boundary, safety_invariants (rldb) |
| `sqlite3_column_bytes` | yes | tested | | storage_class, probe |
| `sqlite3_column_count` | yes | tested | | safety_invariants (rldb) |
| `sqlite3_column_double` | yes | tested | | safety_invariants (rldb) |
| `sqlite3_column_int64` | yes | tested | | upstream_abi, storage_class, probe |
| `sqlite3_column_name` | yes | tested | | safety_invariants (rldb) |
| `sqlite3_column_text` | yes | tested | | storage_class, probe |
| `sqlite3_column_type` | yes | tested | `SQLITE_NULL` is 5, as upstream. | storage_class, probe |
| `sqlite3_column_value` | no | tested | | storage_class, probe |
| `sqlite3_commit_hook` | yes | partial | Fires only from `sqlite3_exec`, only after an explicit `COMMIT` or `END`; a non-zero return does not roll back. | hooks |
| `sqlite3_context_db_handle` | yes | tested | | value_result |
| `sqlite3_create_collation` | yes | tested | Forwards to `sqlite3_create_collation_v2` with no destructor. | collation_register (`_v2`) |
| `sqlite3_create_collation_v2` | yes | tested | | collation_register |
| `sqlite3_create_function` | yes | refuses | `SQLITE_DIRECTONLY` and unknown flag bits fail with `SQLITE_ERROR`. | udf_register |
| `sqlite3_create_function16` | yes | partial | UTF-16LE name; no destructor slot. Same flag refusals. | |
| `sqlite3_create_function_v2` | yes | refuses | As `sqlite3_create_function`; the destructor runs exactly once. | udf_register |
| `sqlite3_create_window_function` | no | refuses | A non-NULL `xValue` or `xInverse` fails with `SQLITE_ERROR`; with both NULL it registers an aggregate. | udf_register |
| `sqlite3_db_filename` | yes | tested | The database directory for `main`, "" for an in-memory database, NULL for any other name. | open_memory, src/tests, probe |
| `sqlite3_db_handle` | yes | tested | | src/tests |
| `sqlite3_db_readonly` | yes | inert | Always 0, also for an unknown schema name (upstream returns -1). | src/tests |
| `sqlite3_errcode` | yes | tested | Extended result codes are never produced. | src/tests, storage_class |
| `sqlite3_errmsg` | yes | tested | | upstream_abi, storage_class |
| `sqlite3_errstr` | yes | tested | | src/tests |
| `sqlite3_exec` | yes | tested | | exec_input_boundary, open_memory, src/tests |
| `sqlite3_extended_result_codes` | yes | inert | Returns `SQLITE_OK`; extended codes are never produced. | |
| `sqlite3_finalize` | yes | tested | | upstream_abi, storage_class, probe |
| `sqlite3_free` | yes | partial | Frees only strings this library returned (`sqlite3_exec` error text, `sqlite3_stats_json`); there is no `sqlite3_malloc`. | src/tests |
| `sqlite3_get_autocommit` | yes | tested | | src/tests |
| `sqlite3_get_auxdata` | no | partial | Data lives only for the current function call, so a later row never sees it. | |
| `sqlite3_interrupt` | yes | partial | Checked only when a step starts, not inside a running step, and never cleared: every later step on the connection returns `SQLITE_INTERRUPT`. | safety_invariants (rldb, NULL only) |
| `sqlite3_is_interrupted` | no | untested | Reports the flag `sqlite3_interrupt` sets. | |
| `sqlite3_last_insert_rowid` | yes | tested | | error_paths, safety_invariants (rldb) |
| `sqlite3_libversion` | yes | partial | RedlineDB's version ("5.1.1" in this release), not a SQLite version. | src/tests, probe |
| `sqlite3_libversion_number` | yes | partial | RedlineDB's version as major×1000000 + minor×1000 + patch. | src/tests |
| `sqlite3_open` | yes | tested | `:memory:` opens a private in-memory database; any other name is a RedlineDB database directory. | open_memory, upstream_abi, probe |
| `sqlite3_open_v2` | yes | refuses | `SQLITE_OPEN_READONLY` fails with `SQLITE_READONLY`; a `file:` name with `SQLITE_OPEN_URI` fails with `SQLITE_CANTOPEN`; the VFS name is not read. | open_memory, src/tests |
| `sqlite3_parameter_count` | yes | redlinedb-only | Upstream's name is `sqlite3_bind_parameter_count`, which is not exported. | safety_invariants (rldb) |
| `sqlite3_prepare_v2` | yes | tested | Reads at most `nbytes` bytes; `*ppStmt` is NULL on failure. | error_paths, bounded_prepare (rldb), probe |
| `sqlite3_prepare_v3` | yes | refuses | Flags other than `PERSISTENT` and `NORMALIZE` fail with `SQLITE_ERROR`. | upstream_abi, src/tests, probe |
| `sqlite3_profile` | yes | partial | Fires only from `sqlite3_exec`. | hooks |
| `sqlite3_reset` | yes | tested | | storage_class |
| `sqlite3_result_blob` | yes | tested | | value_result |
| `sqlite3_result_double` | yes | tested | | value_result |
| `sqlite3_result_error` | yes | tested | | value_result |
| `sqlite3_result_error_code` | yes | tested | | value_result |
| `sqlite3_result_error_nomem` | no | untested | | |
| `sqlite3_result_error_toobig` | no | untested | | |
| `sqlite3_result_int` | yes | tested | | udf_register, value_result |
| `sqlite3_result_int64` | yes | tested | | value_result |
| `sqlite3_result_null` | yes | tested | | value_result |
| `sqlite3_result_subtype` | no | inert | Subtypes are not carried. | |
| `sqlite3_result_text` | yes | tested | | udf_register, value_result |
| `sqlite3_result_value` | no | untested | | |
| `sqlite3_result_zeroblob` | no | untested | | |
| `sqlite3_result_zeroblob64` | no | untested | | |
| `sqlite3_rollback_hook` | yes | partial | Fires only from `sqlite3_exec`. | hooks |
| `sqlite3_set_authorizer` | yes | partial | Asked only table-level `SELECT`, `INSERT`, `UPDATE` and `DELETE`, when a statement steps; never `SQLITE_READ`, DDL, `PRAGMA` or function codes. An invalid return code fails the statement. | hooks |
| `sqlite3_set_auxdata` | no | partial | See `sqlite3_get_auxdata`; the destructor runs when the call returns. | |
| `sqlite3_sourceid` | yes | partial | "redlinedb-ffi" and the version, not a SQLite check-in. | src/tests, probe |
| `sqlite3_sql` | yes | tested | | src/tests |
| `sqlite3_stats_json` | yes | redlinedb-only | Engine counters as JSON; free with `sqlite3_free`. | safety_invariants (rldb) |
| `sqlite3_step` | yes | tested | | open_memory, upstream_abi, storage_class, probe |
| `sqlite3_stmt_busy` | yes | tested | | src/tests |
| `sqlite3_stmt_readonly` | yes | tested | | src/tests |
| `sqlite3_threadsafe` | yes | partial | Returns 1, but a statement must not be used by two threads at once (`docs/compatibility/abi-safety.md`). | src/tests |
| `sqlite3_total_changes` | yes | tested | | src/tests |
| `sqlite3_total_changes64` | yes | tested | | src/tests |
| `sqlite3_trace` | yes | partial | Fires only from `sqlite3_exec`. | hooks |
| `sqlite3_trace_v2` | no | refuses | A callback with a non-zero mask fails with `SQLITE_ERROR`; no event is ever delivered. | hooks |
| `sqlite3_update_hook` | yes | tested | | hooks |
| `sqlite3_user_data` | yes | tested | | value_result |
| `sqlite3_vacuum` | yes | redlinedb-only | Compacts the database (the engine's vacuum). | safety_invariants (rldb) |
| `sqlite3_value_blob` | yes | tested | | value_result |
| `sqlite3_value_bytes` | yes | tested | | value_result, storage_class |
| `sqlite3_value_double` | yes | tested | | value_result |
| `sqlite3_value_dup` | no | tested | | value_result |
| `sqlite3_value_free` | no | tested | | value_result |
| `sqlite3_value_frombind` | no | inert | Always 0. | |
| `sqlite3_value_int` | yes | tested | | value_result |
| `sqlite3_value_int64` | yes | tested | | udf_register, value_result |
| `sqlite3_value_numeric_type` | no | untested | Classifies TEXT by whether it parses as a number; it does not convert the value. | |
| `sqlite3_value_subtype` | no | inert | Always 0. | |
| `sqlite3_value_text` | yes | tested | | value_result, storage_class |
| `sqlite3_value_type` | yes | tested | | value_result, storage_class, probe |

## Database files: native format

A RedlineDB database is a directory in RedlineDB's own storage format (data
file, write-ahead log, catalog). It is not the SQLite file format: SQLite
cannot open it, and RedlineDB does not open SQLite database files. Use a
logical dump to move data between them.

Each structure carries its own version, and a release refuses a version it
does not know instead of misreading it:

- The catalog records `format_version`; a release refuses a catalog newer
  than the one it writes.
- Every B-tree index page carries the index-format epoch (`INDEX_VERSION` in
  `crates/kernel/src/index/mod.rs`). RedlineDB 4.x opens epoch 2, rebuilds
  epoch 1 and refuses anything else ("unsupported format version").
  v5.0.0 writes epoch 3, because index keys changed meaning, so a 4.x release
  refuses a v5 database that has an index.
- A release opening a database written at an older index epoch rebuilds
  those indexes at open, in one transaction. After that the older release
  refuses the database.

There is no downgrade path. Take a physical backup
(`redlinedb backup SRC DST --physical`) before you open a database with a
newer release, and read the release notes first.

## `redlinedb` command line

- The shell options (`redlinedb --help`) are not removed or renamed within
  major 5, and an option keeps its meaning. Options named like the sqlite3
  shell's aim to behave like them; how closely is measured by the parity
  corpus (`docs/sqlite-parity.md`), not promised here.
- Result formatting, messages and the particular non-zero exit code of a
  failure may change in a minor release, for example to match the sqlite3
  shell more closely. Success stays 0 and failure stays non-zero.
- The maintenance subcommands (`stats`, `backup`, `restore`, `archive-check`,
  `replication-slot`, `stream-wal`, `stream-logical`) and their `--json`
  output are experimental. Fields may be added in any release; a removal or
  rename is in the release notes.

## Panics: fail-stop

Release builds use `panic = "abort"`. A panic inside the library or the
`redlinedb` binary aborts the process; the C API never returns an error code
for it. Treat it as a crash: the database is left as after a process kill and
the next open recovers it. `docs/compatibility/abi-safety.md` has the
details, including which argument errors are checked and which abort.
Changing this policy (for example an unwinding build of the C library) is a
recorded decision, not a patch-level change.
