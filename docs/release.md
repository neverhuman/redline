# Release process

`neverhuman/redline` is the release authority for the engine and every included
component. The executable workflows live in the root `.github/workflows/`.
Historical split-repository receipts remain under `subrepos/` as immutable
evidence. They are not configuration or publication prerequisites. The former
forge control plane and its activation policies are retired; all components
use GitHub `neverhuman/redline` and root `subrepos.toml`.

## Required acceptance

Run `just required` locally. It covers engine tests, all declared conformance
suites, central client, web, integration, packaging, security, full-graph dependency review,
and the Jankurai ratchet; none is advisory or soft-gated. Local packaging checks
the current platform. GitHub additionally builds and tests all four native targets.
The branch-protection check `RedlineDB/required` rejects failed, cancelled or
skipped required jobs. Merge with a squash commit to preserve linear history.
Pull requests from forks run only on GitHub-hosted runners and cannot pass
`RedlineDB/required` on their own; `docs/ci-trust-boundary.md` describes the
runner trust boundary, the maintainer run, the host hardening and the canary.

CI uses the included `subrepos/redline-testing` runner and an engine from the
same parent commit. Missing evidence, including `rql_phase1`, and compatibility
regressions fail acceptance. Security reviews every active Cargo lockfile,
checks the inherited cargo-deny policies, scans source for secrets and checks npm
for high-severity advisories. The auditor is downloaded from GitHub and verified
against pinned archive and executable SHA-256 digests.

## Candidate and stable publication

The engine crates are already versioned at 4.1.0. This consolidation publishes
GitHub packages; it does not publish a crates.io chain or modify consumer databases.
Release notes are in `docs/migration/RELEASE_NOTES.md`; the existing engine changelog
retains the development history.

1. Merge the migration PR after every required check passes. Verify required CI
   again on the merged `main` commit, and verify the exact README source commands
   from an anonymous clone and the GitHub source archive.
2. Create the immutable annotated tag `v4.1.0-rc.1` at that verified commit and
   push it. Use a signed tag when a maintainer signing key is configured.
3. `.github/workflows/release-build.yml` runs the complete acceptance workflow,
   generates GitHub build-provenance attestations, then creates and publishes
   the prerelease. It never overwrites an existing release or asset.
4. Verify the published candidate archives and installer, including SQL,
   persistence/reopen, server transactions, FFI, embedded web queries, unsupported
   platforms, checksum rejection and installation paths containing spaces.
5. Create `v4.1.0` at the accepted candidate commit. The same workflow repeats
   acceptance and publishes the stable release. Stable commits must belong to
   `origin/main`. Verify the README's exact installer commands against that release.
6. Mark former component repositories superseded and retire their Redline
   publication paths only after stable acceptance. Preserve their existing tags,
   releases, historical proof records and installed consumer authority.

Do not create a release manually while its workflow is running. If a published
candidate needs a fix, use a new immutable candidate tag and requalify it. Never
move an existing tag or replace a published archive. The current publisher accepts
`v4.1.0` and `v4.1.0-rc.N`; extend that policy deliberately for later versions.

## Packages and provenance

`bash scripts/package-release.sh` with `TAG=v4.1.0-rc.2` produces three archives
for the current native platform. The root `packages.yml` matrix covers Linux
x86_64/ARM64 (glibc 2.35+) and macOS Intel/Apple Silicon (macOS 15+).

- `redlinedb-TAG-PLATFORM.tar.gz`: CLI, server, native libraries and C headers.
- `redline-web-TAG-PLATFORM.tar.gz`: web server with embedded frontend assets.
- `redline-testing-TAG-PLATFORM.tar.gz`: conformance runner, corpus and client smoke tool.

Each archive has a `.sha256` sidecar and contains a CycloneDX SBOM and
`share/redlinedb/build-provenance.json`: one compact JSON line
(`redline.release-build/v2`) recording the repository URL and numeric id, tag,
source commit and tree, platform, package and compiler. The web archive
includes a frontend SBOM; the testing SBOM includes the bundled client. GitHub
release attestations bind the uploaded archive bytes to the workflow identity
and source commit.

`ops/release/authority.env` names the one repository allowed to build and
publish releases: `neverhuman/redline`, id `1390165945`. Packaging refuses to
run when `GITHUB_REPOSITORY_ID` names another repository.
`ops/ci/publish-github-release.sh` refuses unless `GITHUB_REPOSITORY` and
`GITHUB_REPOSITORY_ID` are that repository, refuses any archive whose
provenance names another repository id or tag, and passes
`--repo neverhuman/redline` to every `gh` call, so a mirror or fork running the
same workflow cannot publish. `ops/ci/tests/release-authority.sh` tests both.

Licences: `share/redlinedb/LICENSE` is the Apache-2.0 text and
`share/redlinedb/NOTICE` the project notice. `scripts/release/collect-licenses.sh`
walks the non-dev Cargo dependency graph of the archive's own binaries and
libraries on the build platform (and, for redline-web, the npm production
dependencies) and copies each third-party package's licence, notice and
`license-file` texts into `share/redlinedb/licenses/` (`licenses/npm/` for npm).
`share/redlinedb/DEPENDENCIES.tsv` lists every package in that graph with its
declared licence and where its texts are. Packaging fails if a dependency ships
no licence text, unless `ops/release/license-waivers.toml` waives that exact
version with a reason. `scripts/test-package-licenses.sh` checks the archives
and a fixture of the collector.

The shared library carries the C ABI major (`RLDB_ABI_MAJOR` in
`contracts/c-abi/redlinedb.h`) in its name and load identity:
`lib/libredlinedb.so.5` with soname `libredlinedb.so.5` on Linux and
`lib/libredlinedb.5.dylib` with install name `@rpath/libredlinedb.5.dylib` on
macOS. Archives hold regular files only; packaging refuses symlinks, and the
unversioned development link (`libredlinedb.so`/`libredlinedb.dylib`) is left to
the installer. `scripts/test-package-ffi.sh` checks the entry types and the load
identity, links C consumers against the extracted static and dynamic libraries,
relocates their installation tree and runs both consumers, then runs the
independent ABI probe (`scripts/compatibility/phase2-abi-probe.sh`), which is
compiled only against upstream SQLite 3.53.1's `sqlite3.h`, against both
libraries. macOS libraries are signed again after installation. CI stages release
archives outside the Cargo cache to keep versions separate.

The binary installer defaults to `~/.local` and honors `VERSION` and `PREFIX`.
Without `VERSION` it installs the release that `releases/latest` resolves to,
and reports that there is no published release when GitHub redirects to the
release list instead. Before it writes anything under the prefix it requires a
`.sha256` sidecar with a matching digest (`REDLINEDB_SHA256` adds an
independent pin), an archive of regular files and directories only, and a
`build-provenance.json` naming repository id `1390165945` and the requested
tag. These checks reject corrupt downloads, link entries, and archives built
for another repository or version. They do not verify who built the archive:
whoever can upload a release asset can also upload a matching checksum and
provenance. `REDLINEDB_VERIFY_ATTESTATION=1` adds that check with
`gh attestation verify --repo neverhuman/redline --signer-workflow
neverhuman/redline/.github/workflows/release-build.yml`, which needs an
authenticated GitHub CLI. `install.sh` repeats the repository slug and id from
`ops/release/authority.env` because a piped script cannot read files, and
`scripts/install.sh` is a byte-identical copy; `scripts/test-installer.sh`
checks both. No development toolchain is needed at runtime, and the installer
never replaces `sqlite3`.

## Evidence and rollback

Acceptance artifacts are attached to the CI/release run: conformance evidence,
security receipts, component audit reports and all native package archives.
`docs/migration/inventory.json` and `redlinectl validate --history` verify the
preserved source identities and recovery refs. Earlier audit baselines remain
recorded; policy qualification does not lower their score floors.

If a release is defective, retain its immutable forensic evidence, mark it as
superseded in release notes, and publish a corrected version through the same
acceptance process. Consumers can select a previous verified release with
`VERSION`; back up their data and review format compatibility before changing
installed versions. Release automation does not roll back or replace databases.

Candidate rc.1 exposed an absolute macOS dylib load identity during published-archive verification. Candidate rc.2 corrects the identity and adds extracted-package C consumer relocation tests before stable qualification. The rc.1 tag and assets remain preserved with a known-issue notice.

Candidate rc.3 additionally fixes report generation for declared conformance
skips. Every executed case still needs its configured warmups; required parity
CI now generates a report from the complete evidence before release acceptance.
