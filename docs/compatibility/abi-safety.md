# C ABI safety contract

This page covers what the RedlineDB C library (`libredlinedb`, crate
`redlinedb-ffi`) assumes about its callers and what happens when the library
itself fails. The declarations and pointer lifetimes are in
`contracts/c-abi/redlinedb.h`; the ABI history is in
`docs/exceptions/ffi-c-header.md`.

## Handles and pointers

- A database handle is live from a successful `rldb_open`, `rldb_open_v2`,
  `sqlite3_open` or `sqlite3_open_v2` until it is closed. A statement is live
  from a successful prepare until it is finalized. A backup handle is live
  from `rldb_backup_init` or `sqlite3_backup_init` until it is closed or
  finished. Passing a closed, finalized or foreign pointer is undefined
  behaviour; the library cannot detect it.
- NULL is accepted in place of a handle, string or out-pointer wherever the
  function checks for it, which is every function unless its `# Safety`
  section says otherwise. The result is `SQLITE_MISUSE` or the function's
  documented neutral value.
- A statement, and the text, blob and `sqlite3_value` pointers it returns,
  must not be used by two threads at the same time.
- Input buffers are read only as far as their length says.
  `sqlite3_prepare_v2`, `sqlite3_prepare_v3` and `rldb_prepare_v2` read at
  most `nbytes` bytes when `nbytes >= 0`, stop at the first NUL in any case,
  and read nothing when `nbytes == 0`, so SQL that ends exactly at the end of
  a mapping needs no terminator. `*ppStmt` is NULL after every failure.
  `crates/ffi/tests/bounded_prepare.rs` checks this against a `PROT_NONE`
  guard page, and `scripts/compatibility/phase2-abi-probe.sh` checks it from
  C.

## Rust callers

The crate also builds an `rlib` so its own integration tests can call the
exports. It is not a supported Rust API; Rust programs use the `redlinedb`
crate. Every export that takes a raw pointer is declared
`pub unsafe extern "C" fn` with a `# Safety` section, so safe Rust cannot
reach undefined behaviour through it without an `unsafe` block.
`crates/ffi/tests/unsafe_exports.rs` fails if an export takes a raw pointer
without being `unsafe`, if an `unsafe` export has no `# Safety` section, or if
the crate-wide `allow(clippy::not_unsafe_ptr_arg_deref)` comes back. The C
symbols and calling convention are unchanged by this.

## Panics: the library is fail-stop

The workspace release profile sets `panic = "abort"` (`Cargo.toml`,
`[profile.release]`), and release archives are built with
`cargo build --release -p redlinedb-ffi` (`ops/ci/release-build.sh`). In that
build a panic anywhere in the library, including inside the engine, aborts
the host process with `SIGABRT`. No error code is returned and no further
code in the process runs.

Debug and test builds unwind. There, most entry points run their body inside
`catch_unwind` (`api()` in `crates/ffi/src/util.rs`) and turn a panic into
`SQLITE_INTERNAL` (`RLDB_INTERNAL`, 2). That conversion is a test-build
convenience, not part of the contract: a release library never returns
`SQLITE_INTERNAL` for a panic. A panic that escapes an `extern "C"` function
aborts the process in every build, because Rust does not unwind across that
boundary.

For embedders this means:

- Treat a panic as a crash. The database is left as it would be after a
  process kill, and the next open runs the same recovery.
- Do not rely on `SQLITE_INTERNAL` to detect library bugs, and do not call
  the library from a process that must survive an internal failure; isolate
  it in a worker process instead.
- The argument checks the library does make (NULL handles, unsupported
  flags, out-of-range column and parameter indexes, invalid UTF-8) return
  error codes, not panics. Arguments outside a function's `# Safety`
  contract are not all checked: for example a negative `nbytes` passed to
  `sqlite3_bind_blob` or `rldb_bind_blob` currently panics, and so aborts.

An unwinding release profile for the C library would make `catch_unwind`
effective. It is not adopted for v5.0.0; changing it would be a separate
decision, recorded here and in the release notes.
