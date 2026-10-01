# Install RedlineDB

This is the one installation guide. It covers the release packages, building
from source, the Rust library and the C library. Contributor tooling (just,
nextest, jankurai and the CI lanes) is in [CONTRIBUTING.md](../CONTRIBUTING.md)
and [docs/contributing/tooling.md](contributing/tooling.md).

`scripts/test-docs-quickstart.sh` runs every block marked as the quick start on
this page against each platform's candidate package in CI, with no Rust, C,
Node, just or rtk on `PATH`.

## Release packages

| Platform | Package | Needs |
|---|---|---|
| Linux x86_64 | `redlinedb-<tag>-linux-x86_64.tar.gz` | glibc 2.35 or newer |
| Linux ARM64 | `redlinedb-<tag>-linux-arm64.tar.gz` | glibc 2.35 or newer |
| macOS Intel | `redlinedb-<tag>-macos-x86_64.tar.gz` | macOS 15 or newer |
| macOS Apple Silicon | `redlinedb-<tag>-macos-arm64.tar.gz` | macOS 15 or newer |

A package holds the `redlinedb` shell, `redlinedb-server`, the C library
(`libredlinedb.so.5` or `libredlinedb.5.dylib`, and `libredlinedb.a`), the
headers `redlinedb.h` and `sqlite3.h`, the licences and build provenance.
Running them needs no Rust, C compiler, Node, just or rtk. On an older glibc,
on musl, or on macOS older than 15 the installer stops with a message before
it writes anything; build from source there.

### Quick start

```bash quickstart
curl -fsSL https://raw.githubusercontent.com/neverhuman/redline/v5.1.1/install.sh | VERSION=v5.1.1 bash
export PATH="$HOME/.local/bin:$PATH"
```

The installer URL names the tag, so the script you run is the one released
with that version. It installs into `~/.local`; add the `export` line to your
shell profile to keep `redlinedb` on `PATH`.

Run a first query:

```bash quickstart
redlinedb -batch :memory: 'SELECT 1;'
# prints: 1
```

`:memory:` is a database that disappears when the process exits. A path keeps
the data; the engine creates a directory there holding `data.redline`, `wal/`
and `owner.lock`:

```bash quickstart
workdir=$(mktemp -d)
redlinedb -batch -bail "$workdir/notes.redline" "CREATE TABLE note(id INTEGER PRIMARY KEY, body TEXT NOT NULL); INSERT INTO note VALUES (1, 'hello');"
redlinedb -batch "$workdir/notes.redline" 'SELECT body FROM note WHERE id = 1;'
# prints: hello
```

The second command is a new process, so the row came back from disk.
`redlinedb --help` lists the shell's options; they follow the `sqlite3` shell.
The installer never installs, replaces or aliases a `sqlite3` command.

### Choosing a version, a prefix and a digest

| Variable | Meaning |
|---|---|
| `VERSION` | Release tag, for example `v5.1.1`. Without it the installer asks GitHub for the latest release and stops if there is none. |
| `PREFIX` | Installation root, default `~/.local`. Paths with spaces work. |
| `REDLINEDB_SHA256` | Also require this archive digest (from the release's `.sha256` file). |
| `REDLINEDB_VERIFY_ATTESTATION=1` | Also run `gh attestation verify` against the release workflow of `neverhuman/redline`. Needs an authenticated GitHub CLI. |
| `REDLINEDB_ROLLBACK=1` | Make the previous version active again (below). |
| `REDLINEDB_MIGRATE_LEGACY=1` | Move files of an earlier flat installation aside instead of refusing (below). |
| `REDLINEDB_LOCK_TIMEOUT` | Seconds to wait for another installer on the same prefix, default 300. |

```bash
curl -fsSL https://raw.githubusercontent.com/neverhuman/redline/v5.1.1/install.sh |
  VERSION=v5.1.1 PREFIX="$HOME/redline install" REDLINEDB_SHA256=<sha256 from the release> bash
```

Before anything under `PREFIX` is written, the installer requires the archive
to match its `.sha256` file (and `REDLINEDB_SHA256` when set), to hold only
regular files and directories, and to carry `share/redlinedb/build-provenance.json`
naming repository id `1390165945` (`neverhuman/redline`) and the requested tag.
These checks catch corrupt downloads, link entries and archives built for
another repository or version. They do not show who built the archive: anyone
who can upload a release asset can upload a matching checksum and provenance.
`REDLINEDB_VERIFY_ATTESTATION=1` adds that check.

## Where the files go

```text
PREFIX/lib/redlinedb/versions/v5.1.1/{bin,lib,include,share}   one directory per version
PREFIX/lib/redlinedb/current  -> versions/v5.1.1               the active version
PREFIX/lib/redlinedb/previous -> versions/<the one before>
PREFIX/bin/redlinedb          -> ../lib/redlinedb/current/bin/redlinedb (also redlinedb-server, redlinedb-cli)
PREFIX/lib/libredlinedb.so.5  -> redlinedb/current/lib/libredlinedb.so.5 (macOS: libredlinedb.5.dylib)
PREFIX/lib/libredlinedb.so    -> redlinedb/current/lib/libredlinedb.so   (development link; macOS: libredlinedb.dylib)
PREFIX/lib/libredlinedb.a     -> redlinedb/current/lib/libredlinedb.a
PREFIX/include/redlinedb.h, sqlite3.h -> ../lib/redlinedb/current/include/...
```

The licences, SBOM and build provenance of the active version are in
`PREFIX/lib/redlinedb/current/share/redlinedb/`. `redlinedb --build-info`
(`--json` for one line of JSON) prints the release tag and source commit the
binary was built from, its target, and the repository it was released by.
Headers stay under
`PREFIX/include`. When that directory is `/usr/include` or
`/usr/local/include`, which compilers search by default, `sqlite3.h` is not
linked there, because it would shadow the system SQLite header; use
`-I PREFIX/lib/redlinedb/current/include` to reach it.

### Upgrades, failures and rollback

An install downloads and checks the archive in a temporary directory, copies
it into a staging directory under `PREFIX/lib/redlinedb` (the same
filesystem), and validates the staged copy: `bin/redlinedb --version` must
succeed and `bin/redlinedb -batch :memory: 'SELECT 1;'` must print `1`. Only
then is it renamed to `versions/<tag>`. The stable links in `PREFIX/bin`,
`PREFIX/lib` and `PREFIX/include` all go through `current`, and one rename of
`current` activates the new version. A candidate that fails validation, a
failed copy or rename, or an interrupted install leaves the version that was
active before still active and whole; the CLI, the server, the libraries and
the headers never come from two versions. Installing the tag that is already
installed keeps it and changes nothing.

To go back to the version that was active before:

```bash
curl -fsSL https://raw.githubusercontent.com/neverhuman/redline/v5.1.1/install.sh | REDLINEDB_ROLLBACK=1 bash
```

Rollback validates the previous version the same way, then swaps `current`
and `previous`, so running it again goes forward again. Versions other than
`current` and `previous` can be deleted from `PREFIX/lib/redlinedb/versions/`.
Neither installing nor rolling back touches databases. A database written by a
newer version may not open in an older one; back it up before changing
versions.

### Two installers at once

Installers on the same prefix take turns: each holds the directory
`PREFIX/lib/redlinedb/.lock` (a directory, because `mkdir` is atomic on every
platform; `flock` is not available on macOS) and waits up to
`REDLINEDB_LOCK_TIMEOUT` seconds for another. The lock records its owner in
`.lock/owner`. An installer that is killed with `SIGKILL`, or a machine that
loses power mid-install, leaves the lock behind, and the next installer stops
with a timeout that names the directory. If the recorded process is no longer
running, remove `PREFIX/lib/redlinedb/.lock` and rerun. The installer never
removes a lock it did not take, because it cannot tell a stale lock from a
slow installer on another host that shares the prefix.

### An earlier flat installation

Installers before v5.0.0 copied files straight into `PREFIX/bin`,
`PREFIX/lib` and `PREFIX/include`. The installer does not overwrite a file it
did not create: it lists them and stops before writing anything. With
`REDLINEDB_MIGRATE_LEGACY=1` it keeps exactly those files in
`PREFIX/lib/redlinedb/versions/legacy-<time>/`, keeping their relative paths
(as hard links, or copies where a hard link is not possible), makes that
directory the active version, replaces each flat file with its stable link in
one rename, records the directory as the previous version, and then activates
the new one. A failure or an interrupt at any step leaves every old path
answering with the old file, and rerunning the installer finishes the
migration. `REDLINEDB_ROLLBACK=1` brings the old files back into use. If a
version was already active, it stays the previous version and the legacy
directory is only kept. Other files in the prefix, including an old
`PREFIX/share/redlinedb`, are left alone.

### Removing RedlineDB

```bash
prefix="$HOME/.local"
for link in "$prefix"/bin/* "$prefix"/lib/* "$prefix"/include/*; do
  case $(readlink "$link" 2>/dev/null) in *redlinedb/current/*) rm "$link" ;; esac
done
rm -rf "$prefix/lib/redlinedb"
```

Your databases are wherever you created them; removing RedlineDB does not
touch them.

## Build from source

You need Rust 1.95.0 (`rust-toolchain.toml` selects it when you use rustup), a
C compiler and pkg-config. `scripts/ci-doctor.sh --profile core` checks them.

```bash
git clone https://github.com/neverhuman/redline
cd redline
git checkout v5.1.1
./scripts/build-from-source.sh
./scripts/install-from-source.sh
```

`install-from-source.sh` lays the build out as a package and hands it to the
same installer, so it gets the same layout, validation, lock and rollback: the
build becomes `PREFIX/lib/redlinedb/versions/source-<commit>-<time>-<pid>`.
`PREFIX` chooses the root (default `~/.local`), `CARGO_BUILD_JOBS` limits
build jobs, and `REDLINEDB_MIGRATE_LEGACY=1` works as above. `--all` on both
scripts also builds the testing runner, the release tools and the web
console, which needs Node 22 and npm; start the console with
`redline-web --target-bin "$(command -v redlinedb)"`.

## Rust library

RedlineDB is not published on crates.io. Depend on a release tag and commit
`Cargo.lock` so the build stays reproducible:

```toml
[dependencies]
redlinedb = { git = "https://github.com/neverhuman/redline", tag = "v5.1.1" }
```

The Rust API is not stable yet and may change between releases; see
[api-stability.md](api-stability.md). The embedding example in the
[README](../README.md#embedded-use) is `crates/redlinedb/examples/readme.rs`,
built in CI by `cargo build --locked -p redlinedb --example readme`.

## C library

Programs that `#include <sqlite3.h>` or `"redlinedb.h"` build against an
installation like this:

```bash
cc app.c -I "$HOME/.local/include" -L "$HOME/.local/lib" -lredlinedb -Wl,-rpath,"$HOME/.local/lib" -o app
```

`-lredlinedb` finds the development link, and the program records the ABI
name `libredlinedb.so.5` (`@rpath/libredlinedb.5.dylib` on macOS), so a
future incompatible major is never loaded in its place. The `sqlite3_*`
functions are an experimental subset of SQLite's C API; see
[api-stability.md](api-stability.md) and
[compatibility/abi-safety.md](compatibility/abi-safety.md).

## Troubleshooting

| Message | Meaning |
|---|---|
| `needs glibc 2.35 or newer` / `needs macOS 15 or newer` | The package was not built for this system. Build from source. |
| `no published release in https://github.com/neverhuman/redline` | GitHub has no release to call latest. Set `VERSION`. |
| `cannot download ...` | That tag has no package for this platform, or the network failed. |
| `these paths under PREFIX were not made by this installer` | An earlier flat installation or another program owns them. Use `REDLINEDB_MIGRATE_LEGACY=1` or another `PREFIX`. |
| `candidate failed validation` | The downloaded version does not run here. The active version is unchanged. |
| `timed out ... waiting for .../.lock` | Another installer holds the prefix, or one died. See "Two installers at once". |
