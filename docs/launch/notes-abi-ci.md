# Release notes: C ABI (lane abi-ci)

Draft lines for the v5.0.0 CHANGELOG. The integrator owns `CHANGELOG.md`.

## Breaking: C ABI v5

Programs compiled against the v4 `redlinedb.h`/`sqlite3.h` must be rebuilt
against the v5 headers. There is no v4 compatibility alias.

- `sqlite3_prepare_v3` now takes upstream SQLite's argument order,
  `(db, sql, nbytes, unsigned int prepFlags, ppStmt, pzTail)`. v4 took
  `flags` last as `int`. Only `SQLITE_PREPARE_PERSISTENT` and
  `SQLITE_PREPARE_NORMALIZE` are accepted; other flags return
  `SQLITE_ERROR` ("unsupported sqlite3_prepare_v3 flags") and a NULL
  statement. The header now defines the `SQLITE_PREPARE_*` constants.
- `SQLITE_NULL` is now 5 (it was 0). `sqlite3_column_type`,
  `sqlite3_value_type` and `sqlite3_value_numeric_type` return 5 for NULL.
  `rldb_column_type` still returns `RLDB_NULL` (0). REAL columns now report
  REAL (2); v4 reported INTEGER (1).

- The shared library is now versioned by ABI major: Linux archives ship
  `lib/libredlinedb.so.5` (soname `libredlinedb.so.5`) and macOS archives
  `lib/libredlinedb.5.dylib` (install name `@rpath/libredlinedb.5.dylib`),
  as regular files. Archives no longer contain `libredlinedb.so` or
  `libredlinedb.dylib`; the installer creates that development link, and
  `scripts/install-from-source.sh` creates it too. Programs linked against v4
  (`libredlinedb.so` with no soname) do not load the v5 library by accident.
  The header defines `RLDB_ABI_MAJOR 5`.

## Fixed

- `sqlite3_column_text` returns the text of INTEGER and REAL values (v4
  returned ""), a NULL pointer for NULL (v4 returned ""), and TEXT or BLOB
  bytes including interior NULs. `sqlite3_column_bytes` returns that length
  (v4 returned 8 for NULL, INTEGER and REAL). A TEXT value with an interior
  NUL no longer makes `sqlite3_step` fail with `SQLITE_MISMATCH`.
- `sqlite3_value_text` keeps interior NULs and renders REAL values as
  `CAST(x AS TEXT)` does ("1.0", not "1"), and its pointer stays valid for
  the life of the value.
- `sqlite3_prepare_v2`/`v3` read the SQL only up to the `nbytes` bound or the
  first NUL. v4 ran `strlen` first, so a bounded buffer without a terminator
  could be read past its end. `*ppStmt` is NULL on every failure.
- `sqlite3_open(":memory:")` (and `rldb_open`, `sqlite3_open_v2`,
  `SQLITE_OPEN_MEMORY`) opens a private in-memory database. v4 created a
  database directory named `:memory:` in the working directory.
  `sqlite3_db_filename` reports "" for it. With `SQLITE_OPEN_URI`, a
  `file:` name is refused (`SQLITE_CANTOPEN`) because URI filenames are not
  interpreted.
- A column whose engine-generated name contains a NUL no longer fails
  `sqlite3_prepare_v2`; the name is truncated at the NUL.

## Verification

- `bash scripts/compatibility/phase2-abi-probe.sh`: a C consumer compiled with
  `-Wall -Wextra -Werror` against the vendored upstream SQLite 3.53.1
  `sqlite3.h` (`contracts/c-abi/upstream/`, SHA-256 pinned) runs 15 upstream
  cases and 3 RedlineDB-only cases against the shared and static libraries;
  upstream libsqlite3 is the control for the 15 upstream cases.
- `scripts/test-package-ffi.sh` runs that probe on the extracted archive, and
  checks regular-files-only entries and the soname / install name.
