# FFI C Header Exception

## Surface

- `contracts/c-abi/redlinedb.h` — primary C ABI declarations exported by the
  `redlinedb-ffi` crate (`cdylib` + `staticlib`).
- `contracts/c-abi/sqlite3.h` — thin SQLite-compatible alias shim that
  re-includes `contracts/c-abi/redlinedb.h` (same directory). Filename is
  intentionally `sqlite3.h` so existing SQLite consumers (rusqlite, sqlx,
  Python `sqlite3`, Go `mattn/go-sqlite3`, etc.) can link against
  `redlinedb-ffi` without code changes. It lives next to `redlinedb.h` under
  `contracts/c-abi/` so the entire C ABI surface is one cell outside the Rust
  runtime scan zone.

- `contracts/c-abi/upstream/sqlite-3.53.1/sqlite3.h` — an unmodified copy of
  upstream SQLite's header, pinned by `SHA256SUMS`, and
  `contracts/c-abi/probe/phase2_abi_probe.c`, the C consumer compiled against
  it by `scripts/compatibility/phase2-abi-probe.sh`. Both are probe-only: no
  packaging or install script copies them, and `scripts/test-package-ffi.sh`
  fails if a packaged `sqlite3.h` is not the RedlineDB shim. They live under
  `contracts/c-abi/` for the same audit reason as the headers above.

## Why this is allowed (exception, not the optimal stack)

The optimal stack for this repo is Rust core + TypeScript/React/Vite +
PostgreSQL + generated contracts. A hand-authored C
header file (`.h`) is technically a non-optimal product language artifact.
We keep it because:

1. The C ABI is the published binary contract that every non-Rust consumer
   (Python, Go, Node, Java JNI, R, MATLAB, etc.) compiles against. Removing
   `.h` would force every consumer to maintain their own translation.
2. The header is a *contract surface*, not runtime code. It contains type
   declarations and function prototypes; no business logic, no allocation,
   no I/O. The compiled implementation lives in `crates/ffi/src/`.

## Relocation rationale

Originally the header lived at `crates/ffi/include/redlinedb.h`. The
jankurai stack-language scanner treats anything under `crates/` as Rust
runtime product code, so a hand-authored `.h` file there triggers
`non-optimal-product-language-found` (cap 74) on every audit run. Moving the
file to `contracts/c-abi/redlinedb.h` keeps the contract collocated with
other generated and hand-authored contract artifacts under `contracts/`,
which is the canonical contracts cell in the reference profile and outside
the Rust runtime scan zone.

The `sqlite3.h` shim is consolidated alongside `redlinedb.h` under
`contracts/c-abi/`. The stack-language scanner does NOT exempt the `sqlite3.h`
filename (an earlier note here claiming it did was incorrect — a hand-authored
`.h` anywhere under `crates/` re-fires `non-optimal-product-language-found`),
so the shim is kept under `contracts/c-abi/` like its sibling. It re-exports the
canonical header under the SQLite symbol name via a same-directory include
(`#include "redlinedb.h"`), preserving the binary contract for downstream
consumers. Nothing in the build, tests, or runtime referenced the old
`crates/ffi/include/sqlite3.h` path (the symbol-diff test reads the upstream
`libsqlite3-sys` bundled header, not this shim).

## ABI v5 (the corrected SQLite surface)

The corrections below change the binary contract for callers compiled
against the v4 header, so they ship as ABI major 5 (see "Library naming").
There is no v4 compatibility alias.

- `sqlite3_prepare_v3(db, sql, nbytes, unsigned int prepFlags, ppStmt,
  pzTail)` uses upstream's argument order. v4 declared `flags` last as `int`,
  so a caller compiled against upstream `sqlite3.h` passed its flags where v4
  read the statement pointer. `*ppStmt` is NULL on every failure. PERSISTENT
  (0x01) and NORMALIZE (0x02) are accepted; any other flag (NO_VTAB,
  DONT_LOG, FROM_DDL or an unknown bit) returns `SQLITE_ERROR` with the
  message `unsupported sqlite3_prepare_v3 flags`, because accepting it would
  claim behaviour RedlineDB does not provide. Upstream accepts these flags.
- `SQLITE_NULL` is 5, as upstream. `sqlite3_column_type`,
  `sqlite3_value_type` and `sqlite3_value_numeric_type` return 5 for NULL;
  the native `rldb_column_type` keeps `RLDB_NULL` = 0. Both report the stored
  class (a REAL is 2, not 1) and never change after a conversion.
- `sqlite3_column_text` returns INTEGER and REAL values as the text
  `CAST(x AS TEXT)` produces, TEXT and BLOB values as their bytes (interior
  NULs kept, NUL-terminated), and a NULL pointer for NULL.
  `sqlite3_column_bytes` is the length of that text (the blob length for
  BLOB, 0 for NULL). The pointer is stable until the next step, reset or
  finalize. `sqlite3_value_text` and `sqlite3_value_bytes` follow the same
  rules. An out-of-range column reports `SQLITE_NULL` and records
  `SQLITE_RANGE`.
- `rldb_prepare_v2` reads at most `nbytes` bytes (and stops at the first
  NUL); `nbytes == 0` reads nothing.
- `sqlite3_open(":memory:")`, `rldb_open(":memory:")` and
  `SQLITE_OPEN_MEMORY` open a private ephemeral database; nothing is created
  at the given name and the filename is reported as "". URI filenames are not
  interpreted: with `SQLITE_OPEN_URI` a `file:` name fails with
  `SQLITE_CANTOPEN`; without it the name is a plain path, as in an upstream
  build with URI handling off. Backup into an in-memory destination fails
  with `SQLITE_CANTOPEN`.

## Library naming

`RLDB_ABI_MAJOR` in `contracts/c-abi/redlinedb.h` is the single source of the
ABI major. `crates/ffi/build.rs` reads it and links the cdylib with soname
`libredlinedb.so.<major>` (Linux and other ELF targets) or install name
`@rpath/libredlinedb.<major>.dylib` (macOS). The packaging scripts install the
library under that name as a regular file (`lib/libredlinedb.so.5`,
`lib/libredlinedb.5.dylib`); `scripts/install-from-source.sh` also creates the
unversioned development symlink unless `REDLINEDB_DEV_LINKS=0`, which package
staging sets because archives hold regular files only. A consumer linked with
`-lredlinedb` through that symlink records the ABI-major name, so a later
incompatible major cannot be loaded in its place.

## Maintenance rules

- Edit `contracts/c-abi/redlinedb.h` directly when adding or removing C ABI
  symbols.
- Keep `crates/ffi/src/` exports binary-compatible with the declarations in
  `contracts/c-abi/redlinedb.h`. Any change to a function signature in the
  header must land in the same commit as the matching Rust `extern "C"`
  change.
- Do not move `contracts/c-abi/redlinedb.h` back under `crates/`; the audit
  cap will re-fire.
- Do not move `contracts/c-abi/sqlite3.h` back under `crates/`; the audit cap
  will re-fire. Keep the `sqlite3.h` filename so downstream SQLite consumers
  resolve `#include <sqlite3.h>` against `contracts/c-abi/`.
- A signature or constant change that alters the binary contract raises
  `RLDB_ABI_MAJOR` (and so the soname) in the same change, with a migration
  note.
- The header must compile cleanly with `-Wall -Wextra -Werror` in C and C++.
- Every Rust export that takes a raw pointer is `pub unsafe extern "C" fn`
  with a `# Safety` section; `crates/ffi/tests/unsafe_exports.rs` enforces
  it. The caller contract and the fail-stop (`panic = "abort"`) policy are in
  `docs/compatibility/abi-safety.md`.

## Owner

`c-abi` (see `.jankurai/owner-map.json`).

## Proof lane

`rtk cargo test -p redlinedb-ffi --quiet --locked` exercises the safety
invariants and input-boundary tests that backstop the C ABI surface
declared in the header. `tests/upstream_abi.rs` calls the exported symbols
through declarations written from upstream `sqlite3.h`, not from this header.
`bash scripts/compatibility/phase2-abi-probe.sh` builds this checkout and runs
the independent C probe against the shared and static libraries, with upstream
`libsqlite3` from `target/sqlite-reference/3.53.1` as the control when present;
`scripts/test-package-ffi.sh` runs it on the extracted release archive.
