# Start here

You need a `redlinedb` binary that matches the manual you are reading, then one directory on disk. The engine puts `data.redline`, `wal/`, and `owner.lock` inside that directory.

## Install a release binary

Linux x86_64, Linux ARM64, macOS Intel, and macOS Apple Silicon packages ship the CLI, the server, the native library, and the C headers. Rust is not required to run them. Linux packages expect glibc 2.35 or newer. macOS packages expect macOS 15 or newer; on anything older the installer stops before it writes a file.

```bash
curl -fsSL https://raw.githubusercontent.com/neverhuman/redline/v5.0.0/install.sh | VERSION=v5.0.0 bash
export PATH="$HOME/.local/bin:$PATH"
redlinedb -batch :memory: 'SELECT 1;'
```

The last line prints `1`. The installer URL names the tag, so the script is the one released with that version. The installer checks the archive against its checksum and its build provenance (repository `neverhuman/redline`, the requested tag) before it writes anything, and defaults to `~/.local`. Each version goes in its own directory under `~/.local/lib/redlinedb/versions/`, and `redlinedb`, `redlinedb-server`, the library, and the headers are links through `~/.local/lib/redlinedb/current`. A failed upgrade leaves the previous version in use, and `REDLINEDB_ROLLBACK=1` switches back. It leaves the system `sqlite3` in place.

To choose a prefix:

```bash
curl -fsSL https://raw.githubusercontent.com/neverhuman/redline/v5.0.0/install.sh | \
  VERSION=v5.0.0 PREFIX="$HOME/redline install" bash
```

The space in that prefix is intentional. The installer accepts an install directory whose path contains spaces.

Set `REDLINEDB_SHA256` when you want the installer to require one archive digest. Packages and checksums are on the [GitHub Releases](https://github.com/neverhuman/redline/releases) page. Platform names are `linux-x86_64`, `linux-arm64`, `macos-x86_64`, and `macos-arm64`. [docs/install.md](../install.md) is the full installation guide: attestation checks, the installed layout, rollback, a prefix that an older installer filled, and removal.

This book was written against commit `8ae3a8b79`, an ancestor of the `v5.0.0` tag. `CHANGELOG.md` lists what changed after it.

## Build from source

The toolchain file in the repository asks for Rust 1.95.0. You also need a C/C++ compiler and pkg-config; `scripts/ci-doctor.sh --profile core` checks all three.

```bash
git clone https://github.com/neverhuman/redline
cd redline
git checkout v5.0.0
./scripts/build-from-source.sh
./scripts/install-from-source.sh
```

To build exactly the commit this book describes, check out `8ae3a8b791d4edab88cf8513ad0d99ef709a1202` instead of the tag. `install-from-source.sh` hands the build to the same installer, so it lands in the same versioned layout. `PREFIX` chooses the install root. `CARGO_BUILD_JOBS` limits compile jobs. `--all` on both scripts also builds the testing runner, release tools, and the web console. The console needs Node 22. Start it with `redline-web --target-bin /path/to/redlinedb`.

## A first database from the shell

The shell accepts a database path and SQL on the command line, or SQL on stdin. This is the smallest useful session:

```bash
redlinedb /tmp/notes.redline "CREATE TABLE note (id INTEGER PRIMARY KEY, body TEXT NOT NULL)"
redlinedb /tmp/notes.redline "INSERT INTO note VALUES (1, 'hello')"
redlinedb /tmp/notes.redline "SELECT id, body FROM note"
```

The same statements are in [`examples/first.sql`](examples/first.sql). Dot-commands that the parity harness relies on include `.mode list`, `.headers off`, `.separator`, and `.nullvalue`. Output flags you can pass on the command line include `-json`, `-csv`, `-list`, `-line`, `-header`, `-noheader`, `-bail`, `-echo`, and `-separator`.

`:memory:` creates a database that disappears when the process exits. Use a file path when the rows should still be there after the shell exits.

## The same database from Rust

RedlineDB is not on crates.io. Add the crate by release tag, or by the revision this book describes. Commit `Cargo.lock` in the application so the build stays reproducible.

```toml
redlinedb = { git = "https://github.com/neverhuman/redline", tag = "v5.0.0" }
# or the commit this book describes:
# redlinedb = { git = "https://github.com/neverhuman/redline", rev = "8ae3a8b791d4edab88cf8513ad0d99ef709a1202" }
```

```rust
use redlinedb::Database;

fn main() -> redlinedb::Result<()> {
    let db = Database::create("/tmp/demo.redline")?;
    let mut conn = db.connect()?;

    conn.execute(
        "CREATE TABLE kv(k INTEGER PRIMARY KEY, v TEXT NOT NULL)",
        (),
    )?;
    conn.execute("INSERT INTO kv VALUES (1, 'hello')", ())?;

    let value: String = conn.query_row("SELECT v FROM kv WHERE k = 1", ())?;
    println!("{value}");
    Ok(())
}
```

`Database`, `Connection`, `Statement`, `Transaction`, `Value`, and `Error` are the types most programs need. They are re-exported from the `redlinedb` crate. The public list is the `pub use` block in `crates/redlinedb/src/lib.rs`.

Parameters use the `Params` trait. Pass `()` when the statement has no placeholders. Prefer parameters over string formatting whenever the value came from outside the program.

## Confirm the binary

```bash
redlinedb --version
```

A 4.1.0 build prints `redlinedb v4.1.0 (SQLite 3.45.1 compatibility)`. That parenthetical is the engine's declared SQLite feature level. The official parity oracle is a separate SQLite 3.53.1 shell, built by `scripts/sqlite/build-reference.sh`. Those two version numbers answer different questions. The first is what this build claims. The second is what the corpus was compared against.

`redlinedb --help` prints a SQLite-shaped usage banner that starts `Usage: sqlite3`. That banner is the compatibility text. The binary you ran is still `redlinedb`. With no filename, the shell opens `:memory:` and waits for statements. It does not create a file.

Next: [For agents](03-for-agents.md) if a program will drive the database, [SQL you will write](06-sql-you-will-write.md) for the statements that surprise people, or [SQLite coverage](04-sqlite-coverage.md) if you are moving scripts across from `sqlite3`.
