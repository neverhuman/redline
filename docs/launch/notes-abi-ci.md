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
  `libredlinedb.dylib`; the installer creates that development link inside
  the installed version, and `scripts/install-from-source.sh` installs
  through the same installer. The soname protects programs linked against
  v5 and later from a future incompatible major. It does not protect programs
  linked against v4: those recorded the unversioned `libredlinedb.so`
  (`@rpath/libredlinedb.dylib` on macOS), which now resolves to the v5
  library with its changed `sqlite3_prepare_v3` argument order and
  `SQLITE_NULL` value, so rebuild them against v5. The header defines
  `RLDB_ABI_MAJOR 5`.

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
- For the integrator (not a release-note line): the README's Install section
  (rewritten under DX-03 below) now uses the snippet above, says RedlineDB is
  not on crates.io and links `docs/api-stability.md`. That page's "Database
  files" section states that
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

## Release identity (S8-02)

- Breaking for old archives: `install.sh` now refuses an archive unless its
  `share/redlinedb/build-provenance.json` names repository id `1390165945`
  (`neverhuman/redline`) and the requested tag. Archives built before this
  change carry no repository id, so this installer refuses them.
- `install.sh` also refuses archive entries other than regular files and
  directories (symlinks, hard links, devices), and it reports "no published
  release" when `releases/latest` redirects to the release list; it used to
  fail with `invalid VERSION: vreleases`. A missing release asset is now
  reported by name instead of as a bare curl error. Every refusal happens
  before anything is written under `PREFIX`.
- `REDLINEDB_VERIFY_ATTESTATION=1` makes the installer run
  `gh attestation verify` against the canonical repository's release
  workflow. Checksums and provenance only show that the download is intact
  and was built for this repository and tag; the attestation shows who
  built it.
- Release provenance is now one compact JSON line (schema
  `redline.release-build/v2`) with `repository_url`, `repository_id`, `tag`,
  `commit` and `source_tree`. Packaging refuses to run for another
  `GITHUB_REPOSITORY_ID`, and the publisher refuses unless it runs in
  `neverhuman/redline` (by name and id), refuses archives whose provenance
  names another repository or tag, and passes `--repo neverhuman/redline` to
  `gh` explicitly. `ops/release/authority.env` holds the constants.
- For the integrator (not a release-note line): README, `docs/install.md`
  and `docs/manual/02-start-here.md` now install `v5.0.0` with the
  `install.sh` of that tag. `neverhuman/redline` has no published release
  yet (its `releases/latest` redirects to `/releases`), so those commands
  return 404 until v5.0.0 is published; CI runs them against each
  candidate archive instead.

## Installer: one version at a time, never a mix (DX-04, DX-06)

- Breaking for scripts that read the installed files directly: `install.sh`
  now installs each release into `PREFIX/lib/redlinedb/versions/<tag>/`.
  `PREFIX/bin/redlinedb`, `redlinedb-server` and `redlinedb-cli`,
  `PREFIX/lib/libredlinedb.so.5` (macOS `libredlinedb.5.dylib`), the
  development link `libredlinedb.so` (`libredlinedb.dylib`),
  `libredlinedb.a`, and `PREFIX/include/redlinedb.h` and `sqlite3.h` are
  links through `PREFIX/lib/redlinedb/current`. Licences and provenance of
  the active version are in `PREFIX/lib/redlinedb/current/share/redlinedb/`.
- Fixed: the installer copied the archive straight into the prefix and only
  then ran the new CLI, so a candidate that could not run (a CLI exiting 42
  in the test) or a copy that failed partway left new files, or new and old
  files mixed, in place. Now it verifies the archive in a temporary
  directory, stages it on the prefix's filesystem under a lock, validates the
  staged copy (`--version`; `-batch :memory: 'SELECT 1;'` must print `1`; the
  provenance again), renames it to `versions/<tag>`, and activates it with
  one rename of the `current` link (`mv -T`, or `mv -h` on BSD and macOS,
  chosen by probing). A failure at any step before that rename leaves the
  previous version active and whole.
- `PREFIX/lib/redlinedb/previous` names the version an install replaced, and
  `REDLINEDB_ROLLBACK=1` swaps `current` and `previous` after validating the
  previous version. Installing a tag that is already installed keeps it.
- The installer never overwrites a file it did not create. A prefix that an
  earlier installer filled (4.x copied files flat) is refused, with the list
  of paths, before anything is written; `REDLINEDB_MIGRATE_LEGACY=1` moves
  exactly those files into `versions/legacy-<time>/` and keeps them as the
  previous version, so `REDLINEDB_ROLLBACK=1` brings them back.
- Two installers on one prefix take turns (`mkdir` lock,
  `REDLINEDB_LOCK_TIMEOUT`, default 300 seconds). A lock left by a killed
  installer is reported with its recorded owner and is removed by hand; the
  installer never breaks a lock. Staging left by a killed installer is
  removed by the next one.
- The installer stops before writing anything on glibc older than 2.35 (or
  without glibc, such as musl) and on macOS older than 15, the systems the
  packages are built for, and points to building from source. It refuses an
  archive that ships `bin/sqlite3`, rejects hard links that BusyBox tar lists
  with `->`, and does not link `sqlite3.h` into `/usr/include` or
  `/usr/local/include`, where it would shadow the system SQLite header.
- `scripts/install-from-source.sh` lays the build out as a package and
  installs it through `install.sh`, as
  `versions/source-<commit>-<time>-<pid>`, with the same validation, lock and
  rollback. `--tree DIR` only writes the package tree (package-release.sh
  uses it). `REDLINEDB_DEV_LINKS` and `REDLINEDB_INSTALL_DIR` are gone.
- `scripts/test-installer.sh` now checks all four supported OS/architecture
  pairs (the asset each requests), that every refusal (404, missing
  `.sha256`, wrong `REDLINEDB_SHA256`, another repository's provenance,
  unsupported OS or architecture, old glibc or macOS, and more) leaves the
  prefix byte-identical, a candidate that exits 42 or answers `SELECT 1`
  wrongly, a `cp`, `mv` or `ln` failure injected at each call, two
  concurrent installers, a held lock, rollback, legacy migration and source
  installs. It passes under bash 3.2 (macOS `/bin/bash`) and with BusyBox
  tools. In the `packages` workflow each hosted runner also installs its
  platform's candidate archive with the real `install.sh`
  (`scripts/test-native-install.sh`) and links a C program through the
  installed development link.
- For the integrator (not a release-note line): the macOS `mv -h` path and
  the `@rpath` lookup through the stable links run only on the hosted macOS
  runners of the `packages` workflow; they were not exercised locally.

## Contributor doctor (DX-07)

- `scripts/ci-doctor.sh --profile core|contributor|required` checks the
  tools one lane needs. `core`: rustc as pinned by `rust-toolchain.toml`,
  cargo, a C compiler and pkg-config. `contributor` (`just fast`) adds just,
  cargo-nextest 0.9.133, git, jq and curl; rtk is optional. `required`
  (`just required`) adds Node 22 and npm, Playwright's Chromium, Docker or
  `REDLINE_TESTING_POSTGRES_URL`, the pinned jankurai binary (by SHA-256),
  cargo-audit 0.22.1, cargo-deny 0.19.8 and gitleaks, and stops at once on
  anything other than Linux x86_64, the only platform its pinned binaries
  exist for. The default, and `just ci-doctor`, is `required`.
- The doctor no longer fails on a missing `mold`; the build stopped using it.
- The CI preflight stage runs `ci-doctor.sh --profile core` first and
  `scripts/test-ci-doctor.sh`, which runs every profile against stand-in
  tools.
- `CONTRIBUTING.md` lists the prerequisites of each lane and points to the
  doctor. It named `just security-local`, which never existed; the lane is
  `just security`. The jankurai notes moved to
  `docs/contributing/tooling.md`.

## One install guide that CI runs (L-08, DX-03)

- `docs/install.md` is the installation guide. It held the jankurai
  scaffolding instructions; it now covers the release packages (a quick
  start, `SELECT 1`, a database kept on disk, pinning a version and a digest,
  what the installer checks, the installed layout, upgrades, rollback, the
  lock, migrating a flat install, removal), building from source, the Rust
  git dependency (not on crates.io) and linking the C library.
  `docs/install_redlinedb.md`, which offered the v1.0.1 installer, described
  `install.sh` as a source build and suggested aliasing `sqlite3`, now only
  points to it.
- The README's Install section installs `v5.0.0` from that tag's own
  `install.sh` (it fetched `install.sh` from `main`), runs `SELECT 1` and a
  database on disk with the installed `redlinedb`, and uses CLI commands that
  exist (`stats`, `backup`). It no longer shows `rtk cargo run` or the
  nonexistent `exec` subcommand, and no longer claims crates.io versions.
  Its embedding example is `crates/redlinedb/examples/readme.rs`.
  `docs/manual/02-start-here.md` and `09-operate.md` describe the same
  install, clone and layout.
- `scripts/test-docs-quickstart.sh` runs every block marked
  `bash quickstart` in `docs/install.md` and the README, in a fresh `HOME`
  with `PATH=/usr/bin:/bin` and cargo, rustc, cc, node, npm, just and rtk
  made to fail, against each platform's candidate archive in the `packages`
  workflow, and checks each `# prints:` line (`SELECT 1` must print exactly
  `1`). With `--static`, which the CI preflight runs, it checks that the
  README's Rust example is the example file verbatim and that the active docs
  carry no `jankurai init`, `VERSION=v1.0.1`, `/usr/local/bin/sqlite3`, old
  repository name or installer fetched from `main` (migration records,
  archived pages and these `docs/launch/` drafts may quote them). The preflight also runs
  `cargo build --locked -p redlinedb --example readme`.
- For the integrator (not a release-note line): this lane edited only the
  README's Install, Development Notes and Contributing sections, outside the
  generated blocks. The README-wide rewrite should keep the
  ```` ```bash quickstart ```` and ```` ```rust readme ```` blocks (or move
  them) so the checks keep running.

## Releases: any version, checked before it is built (DX-05, CI-02, CI-05)

- `release-build.yml` now runs for tags `vX.Y.Z` and `vX.Y.Z-rc.N` of any
  version (it ran only for `v4.1.0*`). Its first job,
  `ops/ci/release-version.sh check <tag>`, refuses the tag unless every
  workspace crate has version `X.Y.Z`, `CHANGELOG.md` has a `## [X.Y.Z]`
  section, `docs/releases/vX.Y.Z.md` exists and is not the placeholder, the
  tag is annotated and names the built commit, the repository is
  `neverhuman/redline` by id, the tag has no release yet, and (for a stable
  tag) the commit is on `main`. It reports every failed rule. Malformed tags
  (`v5.0`, `v5.0.0-beta`, `v5.0.0-rc.01`, `v5.0.0-rc.0`) are refused.
  `ops/ci/release-version.sh bump X.Y.Z` gives every workspace crate that
  version through `[workspace.package]` and updates `Cargo.lock`.
- The publisher takes its notes from `docs/releases/vX.Y.Z.md` (it used
  `docs/migration/RELEASE_NOTES.md`, the consolidation notes), marks `-rc.N`
  tags as prereleases, and uploads exactly one archive and one `.sha256` per
  package and platform; a missing, extra or crossed checksum file stops it
  before anything is created. `publish` runs on a GitHub-hosted runner.
- Fixed: a tag push would have failed at startup. `ci.yml`'s `merge-report`
  job asked for `contents: write` and `pull-requests: write`, more than
  `release-build.yml` grants the `ci.yml` it calls, and GitHub checks that
  before any job runs. The job is now `.github/workflows/report-merge.yml`,
  run when the dispatched `ci` run on the report branch succeeds, and the CI
  preflight refuses any `ci.yml` or `packages.yml` permission beyond the
  acceptance grant (`ops/ci/check-workflow-permissions.sh`).
- New `redlinedb --build-info [--json]`: the package version, the release tag
  and source commit the binary was built from, its target, the repository URL
  and id, and the SQLite version of the parity oracle (JSON schema
  `redline.build-info/v1`). Development builds report no tag and source
  `unknown`. Release packaging sets `REDLINEDB_BUILD_TAG` and
  `REDLINEDB_BUILD_SHA`, and `scripts/test-packages.sh` checks that the
  packaged CLI names its archive's tag and commit.
- New `verify-published` job: after publication, on Linux x86_64 and ARM64
  and macOS Intel and Apple Silicon, it installs the release with
  `curl -fsSL https://raw.githubusercontent.com/neverhuman/redline/<tag>/install.sh | VERSION=<tag> PREFIX="…/rl x" bash`,
  runs `SELECT 1`, creates, reopens and reads a database, and checks
  `--build-info --json` against the tag and commit; a stable tag is also
  installed as the latest release (`scripts/release/verify-published.sh`).
  The `packages` workflow runs the same script against each candidate archive
  through a file-transport `curl` (`scripts/test-verify-published.sh`).
- CI builds that are not releases package as `v<workspace version>-dev`
  (`ops/ci/release-version.sh dev-tag`) instead of the fixed `v4.1.0-rc.2`.
- For the integrator (not a release-note line): the Phase 7 bump must run
  `bash ops/ci/release-version.sh bump 5.0.0`, add `## [5.0.0]` to
  `CHANGELOG.md`, and replace `docs/releases/v5.0.0.md`, which is a
  placeholder that the check refuses on purpose. Until the bump,
  `--build-info` reports version 4.1.0 and dev packages are `v4.1.0-dev`.
  `ci.yml` changed, so the parity-report inputs hash changes.

## CI runners (CI-04)

- `RedlineDB/required` and the light jobs (`lint`, `official-evidence-guard`,
  `typecheck`, `test`, `components` testing/central/web/release-tools,
  `security`, `audit`) now run on GitHub-hosted `ubuntu-24.04` for every
  event, so the aggregate never takes a self-hosted slot and the light jobs
  do not depend on the self-hosted runners' links to github.com. The
  required check still waits for the self-hosted jobs it needs. The heavy
  jobs (`preflight`, the `tests` shards, `parity`, `components
  (integration)`) stay on the self-hosted runners.
- Self-hosted jobs no longer fetch the Rust channel manifest in every job:
  `ops/ci/ensure-rust.sh` checks the toolchain `rust-toolchain.toml` pins
  offline and installs only a missing toolchain or component, with retries.
  The jankurai download retries and its verified archive is cached in
  `$RUNNER_TOOL_CACHE/redlinedb-tools/<archive sha256>/`; uploads that run
  after a failure only warn about missing files.
- A push and a `workflow_dispatch` on the same ref no longer cancel each
  other (the concurrency group includes the event).
- The `build` and `cli` jobs, which both ran
  `cargo build --release -p redlinedb-cli`, are gone; packaging builds the
  release CLI on every platform.
- New `tests (kernel-failpoints)` shard: `CI_FAST_STAGE=kernel-failpoints
  bash ops/ci/fast.sh` builds the kernel with `--features failpoints` and runs
  only the failpoint-gated tests (the lib's `failpoints::` tests and every
  `crates/kernel/tests/*.rs` that is `#![cfg(feature = "failpoints")]`) with
  nextest; 16 tests, about 10 s on a warm build. Timeouts: kernel 45 min,
  kernel-failpoints 30 min.
- For the integrator (not a release-note line): the kernel lane may also
  change `ops/ci/fast.sh`'s kernel stage; keep the `kernel-failpoints` case
  name, since `ci.yml`'s matrix and `crates/bench/tests/ci_workflow_routing.rs`
  name it. Maintainer infra (runner egress, more runners) is still needed for
  the heavy jobs to stop flaking.

## Release acceptance manifest (CI-04/CI-06)

- Every release now carries `release-acceptance.v1.json`
  (`redline.release-acceptance/v1`), attested with the archives: the
  repository id, tag, commit, tree, source-inputs hash, compiler, the run and
  every job's conclusion, each archive's sha256, and the digests of the
  receipts it was accepted on (the security receipt, the jankurai security
  evidence, the component audits, the official redline-testing evidence and
  the durability evidence). `scripts/release/verify-acceptance.sh` checks a
  downloaded manifest against the release and the tagged checkout; see
  `docs/release.md`, "Acceptance manifest".
- The CI `security` job now also writes the release security receipt
  (`ops/ci/security-receipt.sh`, full history) and uploads it as the
  `security-receipt` artifact.
- The source-inputs recipe behind `.github/parity-report-inputs.sha256` moved
  to `ops/ci/source-inputs-sha256.sh` (same paths, same hash); redline-testing's
  recipe test reads it there.
- For the integrator (not a release-note line): publishing now requires a
  `durability-evidence` artifact from the tag's `ci.yml` run, and no job
  uploads one yet (`ops/release/acceptance-receipts` says so, and
  `crates/bench/tests/ci_release_acceptance.rs` makes whoever adds the
  uploader update that line). Until the durability receipt job lands in
  `ci.yml`, every tag stops at publish with "missing receipt artifact(s):
  durability-evidence". To release without it, remove its line from
  `ops/release/acceptance-receipts` and from the download pattern in
  `release-build.yml`, and narrow the durability claim instead.

## Parity report bot

- `.github/workflows/sqlite-parity-report.yml` no longer runs daily; it runs
  only when dispatched. The scheduled run re-measured the corpus with
  `--workers auto` on a shared runner after merge and rewrote the README's
  generated blocks from that run. `docs/release.md` ("Parity report") says
  how the report is regenerated for a release.

## Public hygiene and merge policy (DOCS-HYGIENE)

- The chaos benchmark suite has a neutral name: configs
  `crates/bench/bench/chaos.toml`, `chaos-bounded.toml` and
  `chaos-extreme.toml`, the xbabe1 driver `scripts/bench/chaos_xbabe1.sh`,
  suite tag `chaos` in `chaos_report` and in each record's `chaos_suite`, and
  tables `chaos_*`. The one committed chaos result was renamed to
  `benchmark-results/version/25262bf…/suites/chaos-smoke.json` with the same
  substitution inside it; no hash anywhere names that file. Reports written
  before this change carry the old suite tag; pass `chaos_report --suite
  <old tag>` to read them.
- Committed benchmark records no longer carry a personal home path (replaced
  by `<checkout>`), and the xbabe1 bench scripts default `REMOTE_DIR` to
  `RedlineDB` under the remote login directory instead of an absolute host
  path (`xbabe1_run.sh` resolves it to an absolute path for docker).
  `scripts/check-public-hygiene.sh` (CI preflight) keeps both out.
- `docs/testing.md` ("Publication and review") now describes the protection
  `main` must carry on `neverhuman/redline` — strict `RedlineDB/required`,
  one approving review dismissed by a new push, linear history, enforced for
  admins, no force-push or deletion — and one merge method, rebase. It no
  longer lists operator logins, credential files or host paths. Every `gh`
  command in `docs/` names `--repo neverhuman/redline`. `docs/release.md` no
  longer says "merge with a squash commit", and the parity report bot
  (`report-merge.yml`) merges by rebase. `bash ops/release/main-protection.sh
  check|apply` compares or applies that policy.
- For the integrator (not a release-note line):
  - On 2026-09-28 `main-protection.sh check` against the live repository
    reported `main` unprotected (`protected: false`) and squash merges and
    merge commits allowed. A repository admin must run
    `bash ops/release/main-protection.sh apply` (it was not run from this lane).
  - The removed operator table (logins, credential and helper paths) is in
    git history at the parent of this change; move it to the operators'
    private notes if it is still needed.
  - Still carrying the old suite path until regenerated or edited by their
    owner: `CHANGELOG.md` (line ~947, "scripts/bench/<old>_report.py" in the
    history; please reword to "the Python chaos report script"),
    `.jankurai/repo-score.json` and
    `benchmark-results/sqlite-parity/latest/jankurai-score.txt` (next
    `jankurai audit` / report update), and the hash-chained audit baselines
    under `.jankurai/baselines/` and `subrepos/redline/.jankurai/`.
    `scripts/check-public-hygiene.sh` exempts exactly those files.

## Launch claim lint (WP-0.1)

- `scripts/check-launch-claims.sh` (CI preflight, tested by
  `scripts/test-launch-claims.sh`) fails on any tracked line of `README.md`
  or `docs/` (not `docs/archive/` or `docs/migration/`; `docs/releases/`
  included) that makes one of the five launch claims the script names
  (a replacement for SQLite, complete SQLite compatibility, all-safe Rust,
  speed over SQLite), unless that exact line was reviewed as qualified or
  historical in `scripts/launch-claims-allowlist.tsv`.
  Numbers in a reviewed line may change (the generated README blocks); its
  words may not.
- Corrected claims: `docs/architecture/ENGINEERING_SPEC.md` no longer calls
  the engine entirely safe Rust (unsafe code outside the FFI shim is listed in
  `.jankurai/unsafe-ledger.toml`) and says its crash-safety claims are not
  power-loss certified; `docs/exceptions/ffi-unsafe-blocks.md` and
  `docs/issues/redline-jansu-issues.md` no longer present the C ABI or the
  ledger as a SQLite replacement; `docs/architecture.md` calls the Rust
  facade unstable (`docs/api-stability.md`) instead of "stable".
- The manual's Strict durability guarantee (chapters 01 and 07) now says it is
  the design rule checked by the crash-recovery tests, not a power-loss
  certification (new section "Not yet certified" in chapter 07). Chapters 01
  and 04 point at the generated README report for latency ratios instead of
  repeating numbers that go stale when it is regenerated.
- `docs/PHASE10_HANDOFF.md`, `docs/WORKPLAN_slam.md` and
  `docs/WORKPLAN_CLAUDE.md` carry a "historical / internal planning" banner.
- For the integrator (not a release-note line): when Phase 3's durability
  page lands, link it from `docs/manual/07-transactions.md#not-yet-certified`.
  After a README rewrite, rerun `bash scripts/check-launch-claims.sh`; moved
  or reworded claim lines need a new reviewed row (`--print`), and rows for
  deleted lines must be removed.

## Tombstones and errata (docs only)

- `GROK_GAPS.md` gains a "v5.0.0 launch decisions" entry, an erratum for the
  Slice 0 row (2439 + 8 + 4 is 2451, not 2445; the raw run was not kept), and
  a tombstone table for the nine files it names that never reached the
  published history (`docs/compatibility/phase2-abi.md` and the other phase-2
  and cycle-1 notes, `crates/ffi/tests/phase2_abi_probe.c`, the strict-commit
  tests), each with its current equivalent.
- `FEATURE_GAPS.md`'s "partial indexes remain parser-only" line is marked
  historical and points at `docs/sqlite-parity.md`.
- `docs/audit-rubric.md` and `docs/boundaries.md` name the header's current
  path, `contracts/c-abi/redlinedb.h`; `docs/testing.md`'s link to
  `docs/sqlite-parity.md` resolves.

## SQLite feature ledger from typed proof (SQ-07)

- The feature tables in `docs/sqlite-parity.md` are now rendered from
  `docs/sqlite-feature-matrix.json` (`scripts/parity/render-sqlite-feature-matrix.sh`).
  Each row names its status, a proof kind (`semantic_match`, `readback_only`,
  `intentional_reject`, `known_divergence`, `none`), the test functions
  (`path::fn`) behind it, other evidence paths, the official corpus cases it
  covers or fails, and its open subfeatures. `scripts/parity/lint-sqlite-parity-ledger.sh`
  (CI preflight, tested by `scripts/parity/test-lint-sqlite-parity-ledger.sh`)
  fails on a missing test function or path, an unknown case id, a hand edit
  of the tables, a `pass` row with an open subfeature, and typed whole-corpus
  pass counts.
- Corrected rows: `PRAGMA auto_vacuum` was listed as rejected; it is accepted
  and echoes the stored value, which diverges from SQLite once a table exists
  (partial). `PRAGMA wal_checkpoint(MODE)` was listed as rejected; it is
  accepted as a no-op answering (0, 0, 0) (fail). Table-valued PRAGMAs are
  partial: `index_xinfo` omits the rowid key row. Rows whose feature has a
  failing official case now name it and are partial: DML basics (10547,
  10548), savepoints (10560), correlated/scalar subqueries (10502, 10585),
  compound SELECT (10578), foreign keys (10584), aggregates (10586) and
  `PRAGMA query_only` (10253, 10568).
- `crates/sql/tests/sqlite_full_parity.rs` classified 66 bundled-SQLite
  PRAGMAs, but only the row-compared class was checked, and 29 of the 42
  "explicit rejects" (including `auto_vacuum`) are in fact accepted. Every
  class is now tested: row-compared with the oracle (`foreign_key_list`,
  `application_id` and `table_list` added; `schema_version`, which was
  listed but never compared, diverges), accepted without a SQLite value
  claim, a known gap with a fixture that shows it, or rejected with
  "PRAGMA <name> is not supported". The gap helper that returned silently
  when setup or the query failed is split into `assert_redline_rejects` and
  `assert_result_diverges`, both of which require setup to succeed.

## Qualification objects (SQ-08, minimal)

- New CI shard `tests (qualification)` (`CI_FAST_STAGE=qualification bash
  ops/ci/fast.sh`) runs `scripts/qualification/emit-sqlite-qualification.sh`
  and uploads the `sqlite-qualification` artifact: `sqlite_rust_values.json`
  (the `sqlite_full_parity` test results and the SQLite version `rusqlite`
  bundles, 3.50.2 today, not the 3.53.1 reference shell) and
  `sqlite_c_abi.json` (the `redlinedb-ffi` test results and the upstream-header
  ABI probe's per-case receipt). Counts come from the test output; the script
  fails when a test fails, a lane cannot run, or libtest's totals disagree
  with the listed tests. `scripts/qualification/test-emit-sqlite-qualification.sh`
  tests it on fixture logs. Locally (warm build) the shard takes about 20 s.
- Not done (still P1 in the audit): a stateful prepared-statement lane
  (prepare, step, modify, reset through a view) and README badges per lane.

## Package records per component (DX-08)

- The redline-web and redline-testing archives now keep their own records
  (`LICENSE`, `NOTICE`, `VERSION`, `licenses/`, `DEPENDENCIES.tsv`, SBOMs and
  `build-provenance.json`, whose `package` field names them) in
  `share/redlinedb/components/<package>/`. Before, all three archives wrote
  those files to `share/redlinedb/`, so extracting them together left
  whichever came last, and the core's provenance and licence list could name
  another package. The core archive is unchanged (`share/redlinedb/`, which
  `install.sh` reads), and redline-testing's corpus, metadata, schemas and
  templates stay where the runner reads them.
- `scripts/test-packages.sh` extracts each platform's archives in every order
  (`scripts/release/check-package-layout.sh`) and requires
  `share/redlinedb/build-provenance.json` to name package `redlinedb` and every
  package's records to be its own. The publisher, the acceptance manifest,
  `verify-acceptance.sh` and the licence check read each archive's records
  from its own directory (`scripts/release/package-layout.sh`).
- Breaking for scripts that read `share/redlinedb/build-provenance.json`,
  `VERSION` or the licence files from a redline-web or redline-testing
  archive: read `share/redlinedb/components/<package>/` instead.
