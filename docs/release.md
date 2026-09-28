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

Releases are GitHub packages; nothing is published to crates.io, and a release
never modifies consumer databases. A release is an annotated tag `vX.Y.Z`
(stable) or `vX.Y.Z-rc.N` (a candidate, published as a prerelease; `N` counts
from 1). `.github/workflows/release-build.yml`
runs for exactly those tag shapes. Its first job runs
`bash ops/ci/release-version.sh check <tag>`, which refuses the tag unless:

- every crate of the root workspace has version `X.Y.Z`
  (`bash ops/ci/release-version.sh bump X.Y.Z` sets them, and the Cargo.lock);
- `CHANGELOG.md` has a `## [X.Y.Z]` section;
- `docs/releases/vX.Y.Z.md` exists and is not the placeholder; it is the
  release notes of the stable release and of every candidate;
- the tag is annotated and names the commit being built;
- the repository is `neverhuman/redline` by numeric id;
- the tag has no release yet;
- a stable tag's commit is on `origin/main`.

Run the same check locally before pushing a tag; it reports every failed rule.

1. Merge after every required check passes. Verify required CI again on the
   merged `main` commit, and verify the exact README source commands from an
   anonymous clone and the GitHub source archive.
2. Bump the crates, write the changelog section and the release notes, and
   merge that. Create the annotated tag `vX.Y.Z-rc.1` at the verified commit
   (`git tag -a`; sign it when a maintainer signing key is configured) and push
   that one tag.
3. `release-build.yml` runs the complete acceptance workflow (`ci.yml`, which
   may ask for no more than `contents: read` and `attestations: read`), writes
   and checks the acceptance manifest (below), generates GitHub
   build-provenance attestations for the archives and the manifest, and
   publishes the prerelease with those notes from a GitHub-hosted runner. It
   never overwrites an existing release or asset, and it uploads exactly one
   archive and one checksum for each package and platform, plus
   `release-acceptance.v1.json`.
4. `verify-published` then installs the candidate on Linux x86_64 and ARM64
   and macOS Intel and Apple Silicon from the public installer URL with
   `VERSION` pinned (a prerelease is never `releases/latest`), runs
   `SELECT 1` and a database that survives reopening, and checks that
   `redlinedb --build-info --json` names the tag and the tagged commit
   (`scripts/release/verify-published.sh`; the `packages` workflow runs the
   same script against each candidate archive before publication). Verify by
   hand what it does not cover: server transactions, FFI, embedded web
   queries, unsupported platforms and checksum rejection.
5. Create `vX.Y.Z` at the accepted candidate commit. The same workflow repeats
   acceptance and publishes the stable release; `verify-published` also
   installs it as the latest release. Verify the README's exact installer
   commands against that release.
6. Mark former component repositories superseded and retire their Redline
   publication paths only after stable acceptance. Preserve their existing tags,
   releases, historical proof records and installed consumer authority.

Do not create a release manually while its workflow is running. If a published
candidate needs a fix, use a new immutable candidate tag and requalify it. Never
move an existing tag or replace a published archive.

## Packages and provenance

`bash scripts/package-release.sh` with `TAG=<tag>` produces three archives
for the current native platform; CI builds that are not releases use
`v<workspace version>-dev` (`bash ops/ci/release-version.sh dev-tag`). The
root `packages.yml` matrix covers Linux x86_64/ARM64 (glibc 2.35+) and macOS
Intel/Apple Silicon (macOS 15+).

- `redlinedb-TAG-PLATFORM.tar.gz`: CLI, server, native libraries and C headers.
- `redline-web-TAG-PLATFORM.tar.gz`: web server with embedded frontend assets.
- `redline-testing-TAG-PLATFORM.tar.gz`: conformance runner, corpus and client smoke tool.

Each archive has a `.sha256` sidecar and contains a CycloneDX SBOM and
`share/redlinedb/build-provenance.json`: one compact JSON line
(`redline.release-build/v2`) recording the repository URL and numeric id, tag,
source commit and tree, platform, package and compiler. The CLI reports the
same tag and commit: `redlinedb --build-info --json` prints one line
(`redline.build-info/v1`) with the package version, tag, source commit,
target, repository URL and id, and the SQLite version of the parity oracle.
The web archive includes a frontend SBOM; the testing SBOM includes the bundled client. GitHub
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

Installation is failure-atomic. Each version lives in
`PREFIX/lib/redlinedb/versions/<tag>/`; the stable paths in `PREFIX/bin`,
`PREFIX/lib` and `PREFIX/include` are links through
`PREFIX/lib/redlinedb/current`. A candidate is staged on the prefix's
filesystem under a `mkdir` lock, validated from the staged copy (`--version`,
`SELECT 1` must print `1`, provenance), renamed into `versions/<tag>`, and
activated by renaming a new `current` link over the old one (`mv -T` or BSD
`mv -h`, chosen by probing). `previous` keeps the version it replaced and
`REDLINEDB_ROLLBACK=1` swaps them. Files the installer did not create are never
overwritten; `REDLINEDB_MIGRATE_LEGACY=1` moves a pre-v5 flat installation into
`versions/legacy-<time>/` first. `scripts/install-from-source.sh` activates
source builds through the same installer. `docs/install.md` is the user guide.

`scripts/test-installer.sh` exercises the installer on every OS/architecture
pair with shims (asset names, latest resolution, every refusal leaving the
prefix byte-identical, a candidate that exits 42, a cp/mv/ln failure injected
at every call, two concurrent installers, rollback, legacy migration, the glibc
2.35 and macOS 15 floors). On each platform's hosted runner
`scripts/test-native-install.sh` installs the real candidate archive with
`install.sh` through a file-transport curl and links a C program against the
installation, and `scripts/test-docs-quickstart.sh` runs the quick start of
`docs/install.md` and `README.md` against it.

## Acceptance manifest

Every release carries `release-acceptance.v1.json` (schema
`redline.release-acceptance/v1`), which binds the release to the run that
accepted it. The publish job downloads the tag run's package archives and the
receipt artifacts that `ops/release/acceptance-receipts` lists, and
`ops/ci/release-acceptance.sh` records:

- the repository slug and id, the tag, the commit it names, that commit's
  tree, the source-inputs hash (`ops/ci/source-inputs-sha256.sh`, the hash
  `.github/parity-report-inputs.sha256` holds for the committed report),
  whether the checkout was clean, and the compiler every package names;
- the run id, attempt and URL, and every completed job of that attempt with
  its conclusion (`gh api .../runs/<id>/attempts/<attempt>/jobs`);
- each archive's name and sha256;
- each receipt's name, file count and digest (the sha256 of its sorted
  `sha256  ./path` lines): `security-receipt` (`ops/ci/security-receipt.sh`
  in the `security` job), `security-evidence`, `audit-family`,
  `redline-testing-official-evidence` (the `parity` job) and
  `durability-evidence`.

It refuses when a receipt is missing, so the publish job cannot publish a
release whose run did not upload all of them. No `ci.yml` job uploads
`durability-evidence` yet: until one does, every tag stops at publish.
`scripts/release/verify-acceptance.sh` then checks the manifest fail-closed:
the canonical repository, every job concluded success and a
`RedlineDB/required` job among them, the exact twelve archives with matching
digests and build provenance (repository id, tag, commit, tree, compiler),
the tree and source-inputs hash of the checkout, a clean checkout, the
receipts' digests, and a passing security receipt for the commit. The publish
job runs it before attesting the manifest, and
`ops/ci/publish-github-release.sh` runs it again and refuses to publish
without it. To check a published release:

```bash
git fetch origin tag vX.Y.Z && git checkout vX.Y.Z
gh release download vX.Y.Z --repo neverhuman/redline --dir release
gh attestation verify release/release-acceptance.v1.json --repo neverhuman/redline
bash scripts/release/verify-acceptance.sh release/release-acceptance.v1.json \
  --packages release --tag vX.Y.Z
```

The receipts themselves stay artifacts of the run (GitHub keeps them for the
repository's artifact retention period); the manifest keeps their digests.

## Parity report

The README's generated blocks (the SQLite parity badge, report and metrics,
and the PostgreSQL parity block), `benchmark-results/sqlite-parity/latest/`
and the report charts are written only by `redline-testing report` and
`redline-testing check-postgres` from hash-verified evidence; never edit them
by hand. `.github/workflows/sqlite-parity-report.yml` no longer runs on a
schedule: its daily run measured the corpus with `--workers auto` on a shared
self-hosted runner after merge and rewrote the README from that run, not from
the evidence a release was accepted on. The report changes only when a
maintainer regenerates it:

- For a release, regenerate it from the candidate's own evidence, measured in
  a clean clone of the candidate commit with the release CLI built alone:
  `bash ops/ci/sqlite-parity-report.sh update` runs the official lane,
  renders the blocks and checks the PostgreSQL block against the commit
  (`--expected-source-commit`, `--require-clean`). Commit the outputs with
  `bash ops/ci/source-inputs-sha256.sh > .github/parity-report-inputs.sha256`
  and confirm `just sqlite-parity-report-check` reports no drift. If the
  source inputs change after that, the evidence no longer describes the
  candidate: measure again. `release-acceptance.v1.json` records the tag's
  source-inputs hash, so the two can be compared.
- When only the renderer changed, render again from the committed evidence
  without measuring: `target/release/redline-testing report` with the
  arguments `scripts/just/run.sh` passes for `sqlite-parity-report-update`,
  then `just sqlite-parity-report-check`.
- Or dispatch the bot: `gh workflow run sqlite-parity-report.yml --repo
  neverhuman/redline`. It measures on a self-hosted runner, skips when
  `.github/parity-report-inputs.sha256` already matches, defers while another
  pull request is open, and otherwise opens the report pull request, which
  `report-merge.yml` merges once `ci` passes on its exact head.

## Evidence and rollback

Acceptance artifacts are attached to the CI/release run: conformance evidence,
security receipts, component audit reports and all native package archives,
bound to the release by `release-acceptance.v1.json` (above).
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
