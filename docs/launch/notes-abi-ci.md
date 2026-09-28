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

## Breaking: registrations, hooks and flags fail closed

- A function or collation registered on a connection is removed when the
  connection closes. Before, the process-wide registry kept it under the
  handle's address, so a connection opened later at the same address could
  call the closed connection's callback with its freed `user_data`.
  `sqlite3_collation_needed` is now per connection (it was process-wide).
- `sqlite3_create_function_v2` destructors run exactly once: when the
  function is replaced, when it is deleted, when the connection closes, or
  when the registration fails. Before, they never ran. Collation
  destructors run on replace, delete and close (not on failure, as upstream).
- Deleting a function (every callback NULL) returns `SQLITE_OK`; it returned
  `SQLITE_MISUSE`. Passing `xFunc` with `xStep`/`xFinal` is now
  `SQLITE_MISUSE`; it registered a scalar.
- `SQLITE_DIRECTONLY` and unknown `enc` bits make `sqlite3_create_function*`
  fail with `SQLITE_ERROR`; they were accepted and ignored.
  `SQLITE_DETERMINISTIC`, `SQLITE_INNOCUOUS`, `SQLITE_SUBTYPE` and
  `SQLITE_RESULT_SUBTYPE` are accepted and recorded.
- `sqlite3_create_window_function` with `xValue` or `xInverse` fails with
  `SQLITE_ERROR`; they were silently dropped.
- `sqlite3_trace_v2` with a callback and a non-zero mask fails with
  `SQLITE_ERROR`; it returned `SQLITE_OK` and never called back.
- An authorizer return code other than OK, DENY or IGNORE fails the statement
  with `SQLITE_ERROR` "authorizer malfunction"; it was treated as OK. The
  authorizer's database name moved from `arg4` to `arg5`, as upstream.
- `sqlite3_blob_open` with `flags == 0` is read-only: `sqlite3_blob_write`
  returns `SQLITE_READONLY`. It used to write.
- `docs/security-capabilities.md` lists what the C ABI enforces, refuses and
  does not implement yet (among them: no `SQLITE_READ` authorizer calls, no
  trace_v2 events, trace/profile/commit hooks only from `sqlite3_exec`).

## Breaking: `rldb_config` is checked

- `rldb_open_v2` reads `struct_size` first and never reads past it, so a
  program built against a shorter struct is not read out of bounds. It
  returns `RLDB_MISUSE` (no handle, nothing created) when `struct_size` is
  below 4, above 4096 or ends inside a field, when `flags` is not 0, when
  `durability` is not 0, 1 or 2, or when a byte past the known fields is not
  zero. v4 read the whole struct and ignored `flags` and `durability`.
- `durability` now takes effect: the header defines
  `RLDB_DURABILITY_DEFAULT` (0, Strict), `RLDB_DURABILITY_STRICT` (1) and
  `RLDB_DURABILITY_NORMAL` (2, `PRAGMA synchronous` reads back 1).
- A zero field keeps its default. v4 copied zeros over the defaults, so a
  zero-initialised config set the query memory and spill budgets to 0 and
  every sort failed; it also zeroed the statement cache and busy timeout.
- `rldb_backup_init` refuses a non-NULL `dst_config` with `RLDB_MISUSE`; v4
  ignored it.

## API stability

- New `docs/api-stability.md` states what each interface promises: the Rust
  crates are unstable and not published; `rldb_*` and `redlinedb.h` are stable
  within ABI major 5; the `sqlite3_*` subset is experimental, with a
  per-symbol status table; the database format is native and versioned; the
  shell's option names are kept within 5.x; panics abort.
  `crates/ffi/tests/api_stability.rs` keeps the table and the `rldb_*` header
  in step with the exports.
- Every workspace crate now has `publish = false`. RedlineDB is not on
  crates.io; depend on a git tag
  (`redlinedb = { git = "https://github.com/neverhuman/redline", tag = "v5.0.0" }`).
  `scripts/check-publish-policy.sh` runs in the CI preflight.
- The `redlinedb-tokio` and `redlinedb-sqlx` manifests point `repository` at
  `https://github.com/neverhuman/redline`.
- For the integrator (not a release-note line): `README.md` still says
  "Existing crates.io versions remain available" (there are none) and its
  Rust dependency snippet still pins tag `v4.1.0`. The README rewrite should
  use the snippet above and
  link `docs/api-stability.md`. Its "Database files" section states that
  v5.0.0 writes index-format epoch 3 and that 4.x refuses such a database;
  that is the sql lane's commit 4c5141381 (`INDEX_VERSION` 2 to 3), so the
  sentence is true only once that commit is merged. `docs/testing.md` still
  lists a `cargo yank` rollback step, which no longer applies.

## Supply chain and secret scanning

- Every tracked `Cargo.lock` outside test fixtures is now scanned: the CI
  security job derives the list with `git ls-files`, where it used to be a
  typed list that skipped redline-central's `cargo deny` (it had no
  `deny.toml`) and missed the two nested tools. Each needs its own
  `deny.toml`, passed with `--config`, or an entry with a reason in
  `ops/ci/security-exemptions.tsv`. redline-central and both nested tools now
  have one.
- Every `deny.toml` denies yanked crates, unknown registries, unknown git
  sources and wildcard versions (path dependencies of `publish = false` crates
  stay allowed). redline-central's crates are now `publish = false` too.
- Two lockfile moves the new policy required: `chacha20` 0.10.0 and 0.10.1
  (yanked) to 0.10.2 in the root and redline-central lockfiles, and
  `event-listener` 5.4.1 to 5.4.2 (RUSTSEC-2026-0221, unsound `StackSlot`
  Send/Sync, reached through sqlx-core by `redlinedb-sqlx`).
- The gitleaks allowlist tests the matched text instead of the whole line, so
  a secret next to an allowlisted checksum is reported.
- `ops/ci/security-receipt.sh` writes `target/security/receipt.json` for a
  release candidate (see `docs/security-scans.md`). A full-history scan of
  the candidate found no leaks.

## Safety contract and panics

- The caller contract for handles, pointers and input lengths, and the panic
  policy, are written down in `docs/compatibility/abi-safety.md`. Release
  builds of the C library use `panic = "abort"`: an internal panic aborts the
  host process instead of returning `SQLITE_INTERNAL`. Treat the library as
  fail-stop.
- Rust only (no C ABI change): the `redlinedb-ffi` exports that take raw
  pointers are now `unsafe fn`, each with a `# Safety` section, and the unused
  hidden helper `sqlite3_api::collation::__test_consume_buffer` is gone. The
  crate's `rlib` exists for its own tests and is not a supported Rust API.

## Verification

- `bash scripts/compatibility/phase2-abi-probe.sh`: a C consumer compiled with
  `-Wall -Wextra -Werror` against the vendored upstream SQLite 3.53.1
  `sqlite3.h` (`contracts/c-abi/upstream/`, SHA-256 pinned) runs 15 upstream
  cases and 3 RedlineDB-only cases against the shared and static libraries;
  upstream libsqlite3 is the control for the 15 upstream cases.
- `scripts/test-package-ffi.sh` runs that probe on the extracted archive, and
  checks regular-files-only entries and the soname / install name.

## Licensing and security reporting

- `LICENSE` is now the complete Apache License 2.0 text (it held only the
  short application notice, so GitHub could not identify the licence), and a
  `NOTICE` file names the project and its SQLite attribution. Both ship in
  every release archive under `share/redlinedb/`.
- Release archives now carry the licence texts of the third-party code they
  contain. `scripts/release/collect-licenses.sh` follows the non-dev
  dependency graph of each archive's own binaries and libraries on the build
  platform (before: every package in the lockfile, including dev and bench
  dependencies and other platforms' crates, with empty licence folders for
  48 of the 318 in the core archive), honours each crate's `license-file`,
  also copies licence files of bundled C sources and vendored code, and adds
  the npm production dependencies of the web console under
  `share/redlinedb/licenses/npm/`. Packaging fails when a dependency has no
  licence text unless `ops/release/license-waivers.toml` waives that exact
  version with a reason (one waiver today: npm `victory-vendor` 36.9.2).
- `share/redlinedb/DEPENDENCIES.tsv` now has a header row and four more
  columns: `ecosystem`, `source`, `license_texts` and `waiver`.
  `sbom.cdx.json` lists the same shipped graph instead of the whole lockfile.
- `SECURITY.md` names a working private route: GitHub private vulnerability
  reporting at https://github.com/neverhuman/redline/security/advisories/new.
  It adds supported versions (5.0.x; 4.x unsupported from 5.0.0), scope,
  response targets (acknowledgement in 3 business days, triage in 10,
  90-day default disclosure) and safe-harbor terms. The new-issue page links
  the same form.
- Maintainer step before launch (not a release-note line): enable private
  vulnerability reporting with
  `gh api -X PUT repos/neverhuman/redline/private-vulnerability-reporting`
  and confirm `gh api repos/neverhuman/redline/private-vulnerability-reporting --jq .enabled`
  prints `true`; until then the advisory form refuses reports.

## CI trust boundary (S8-06)

- Pull requests from forks now run only on GitHub-hosted runners, each job
  with its own `CARGO_HOME` under `$RUNNER_TEMP`. Before, every CI job ran on
  the self-hosted runners and shared their cargo cache and its `bin` on
  `PATH` with the trusted jobs that publish releases. The Postgres parity
  lane and the kernel test stage stay on the self-hosted runners, so for a
  fork pull request `RedlineDB/required` fails with "Maintainer run required"
  until a maintainer runs CI on a reviewed copy of the commit.
- cargo-nextest is downloaded as one pinned release, checked against a pinned
  SHA-256 and installed into a job-local directory
  (`ops/ci/install-nextest.sh`). CI used to pipe an unverified download into
  `tar`, and skipped even that when any `cargo-nextest` was on `PATH`.
- Every workflow checkout drops its token (`persist-credentials: false`)
  except the parity report job, which pushes its branch.
- New self-hosted runner hook `ops/ci/runner-job-started.sh` refuses jobs for
  pull requests from forks, which the workflow files alone cannot prevent
  because a pull request runs its own copy of them.
  `docs/ci-trust-boundary.md` covers it, the fork merge procedure, the
  maintainer host work and the canary.
- Maintainer step before launch (not a release-note line): deploy the hook on
  both self-hosted hosts, move the runners to a user with no credentials,
  wipe the shared caches once, and run the canary in
  `docs/ci-trust-boundary.md`. The release `publish` job still runs on the
  self-hosted runners without `environment: release`; Phase 6 moves it.

## Release authority (DX-01)

- `https://github.com/neverhuman/redline` is the one source, review and
  release authority in every active file: install commands, clone commands,
  the Rust git-dependency snippets, `subrepos.toml`, `PUBLIC_MONOREPO.toml`,
  the runner installer default, the release smoke URL and every included
  component's README, AGENTS and release notes. The old repository name
  (`RedlineDB` under the same owner) now resolves to a different owner's
  repository. Historical records (`CHANGELOG.md`, `docs/migration/`,
  `subrepos/redline/`, `tips/`, release evidence, score history) keep it.
- `redline-proof validate` (the release-tools CI lane) requires the
  canonical fetch and push remotes, and it fails when any active shell,
  TOML, YAML, Rust, AGENTS.md, README.md or `docs/` page names the old
  repository in any letter case. This fixes the `components (release-tools)`
  CI failure, where validate demanded the old repository as `origin`.
- The release smoke (`just release-binary-smoke`) now labels its local
  package `v5.0.0` instead of the legacy `v2.0.6`, and downloads from the
  canonical repository when told not to build locally.
- Maintainer step (not a release-note line): a checkout that still has a
  remote pointing at the old repository (the integrator checkout's
  `origin-disabled`) fails `redline-proof validate` until that remote is
  removed. Runners registered against the old repository must be
  re-registered against `neverhuman/redline`.
