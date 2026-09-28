# Upstream SQLite 3.53.1 `sqlite3.h` (probe-only)

`sqlite3.h` is an unmodified copy of the header that
`scripts/sqlite/build-reference.sh` installs from `sqlite-src-3530100.zip`
(SHA3-256 `27cfc9264b2188fd17f811a8c03424eb65391c2ef9874cbfc860ea25f4322363`),
`SQLITE_SOURCE_ID` `2026-05-05 10:34:17 c88b22011a54b4f6fbd149e9f8e4de77658ce58143a1af0e3785e4e6475127e9`.
Its SHA-256 is recorded in `SHA256SUMS` and equals
`artifacts["include/sqlite3.h"]` in the reference build's
`oracle-identity.json`. SQLite is in the public domain.

It exists so `scripts/compatibility/phase2-abi-probe.sh` can compile a C
consumer against upstream declarations instead of RedlineDB's own header. It is
never installed or packaged: the packaging scripts copy only
`contracts/c-abi/redlinedb.h` and the `contracts/c-abi/sqlite3.h` shim, and
`scripts/test-package-ffi.sh` fails if a packaged `sqlite3.h` is not the shim.
Do not edit it; replace it only together with the pinned reference version.
