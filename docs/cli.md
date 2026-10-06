# v5.1.1 CLI and configuration

This reference was checked against the published `v5.1.1` core archive,
whose build-info names commit
`9277455d5ad008252053a81d18add39b8cdc8f7b`. The
[documentation check](documentation-checks.md) pins the archive digest and
tests the examples against that executable.

## Invocations

Use `redlinedb [OPTIONS] [DATABASE_DIRECTORY [SQL]]`, or send SQL on stdin.
`:memory:` and an omitted path create a process-local database. On-disk
paths name RedlineDB directories, not SQLite database files. `--help`
retains a SQLite-shaped banner starting `Usage: sqlite3`; that text does not
change the database format.

```bash doctest
redlinedb -batch :memory: 'SELECT 1;'
# prints: 1
```

Long shell options accept either a single or double dash, such as `-batch`
and `--batch`. This does not imply single-letter aliases. Use `--help` and
`--version` for the CLI; the server also accepts `-h` for help.

The maintenance forms are experimental:

| Command | Arguments |
| --- | --- |
| `backup` | `SRC DST [--logical\|--physical]` |
| `restore` | `BACKUP DST [--target-lsn N\|--target-csn N\|--latest]` |
| `archive-check` | `DB [--json]` |
| `replication-slot` | `create\|drop\|list DB NAME [--physical\|--logical] [--json]` |
| `stream-wal` | `DB SLOT` |
| `stream-logical` | `DB SLOT [--ndjson]` |
| `stats` | `DB [--json]` |

Use `redlinedb stats --help` for the complete maintenance usage. A separate
maintenance process must not open a directory another process owns; call
the backup API within the owning process, or quiesce it first.

## Output and execution options

The output modes are `-list`, `-line`, `-column`, `-box`, `-table`, `-csv`,
`-json`, `-html`, `-ascii`, `-markdown`, `-quote`, `-tabs` and `-tcl`.
`-header`/`-noheader`, `-separator SEP`, `-newline SEP`, `-nullvalue TEXT`
and `-escape T` adjust output. Mode flags reset their own defaults, so option
order matters. CSV emits CRLF rows in v5.1.1, including when `-newline`
requests another row separator:

```bash doctest crlf
redlinedb -csv -header -batch :memory: "SELECT 1 AS id, 'Ada' AS name;"
# prints: id,name
# prints: 1,Ada
```

```bash doctest
redlinedb -json -batch :memory: "SELECT 1 AS id, 'Ada' AS name;"
# prints: [{"id":1,"name":"Ada"}]
```

```bash doctest
redlinedb -list -separator ';' -nullvalue NULL -batch :memory: 'SELECT 1, NULL;'
# prints: 1;NULL
```

`-bail` stops a stdin script at its first error; `-echo` echoes input.
`-init FILE` runs an initialization script and repeated `-cmd SQL` options
run commands before the main input. `-error-exit N` chooses the nonzero SQL
error exit status. `-safe` refuses shell/system dot-commands, and `-nonce`
configures the safe-mode bypass token. `-readonly` rejects SQL writes but
does not make recovery against a live writer safe; see
[owner-lock exceptions](manual/09-operate.md#one-writer-process-many-connections).
`-ifexists` refuses creation of a missing on-disk directory.

`--build-info --json` prints the release identity. `--rql` reads a relational
JSON document instead of SQL and refuses SQL arguments or `--cmd` in that
invocation. `-shellzero` and `-no-shellzero` select the short-lived in-memory
execution path. `.help` can show a reduced list on that fast path; use
`--no-shellzero :memory: .help` to inspect the full dot-command listing.

## Compatibility options with limits

- `-heap N MIN` in the shipped help is inaccurate. The parser consumes
  only `N`; supplying `MIN` can make it the database filename. The accepted
  heap argument does not impose a memory limit.
- `-mmap N`, `-lookaside N M`, `-vfs NAME`, `-maxsize N`, `-append`,
  `-utf8` and `-no-utf8` are accepted without configuring the corresponding
  SQLite facility. `-pagecache N M` emits a compatibility notice, not a
  page-cache allocation policy. Use the engine's Rust `OpenOptions` for
  its supported memory settings.
- `-nofollow` refuses a missing path but follows an existing symlink. Do
  not use it to enforce a no-symlinks boundary.
- `-interactive` without SQL prints a banner/prompt stub and returns; it
  does not run SQLite's interactive loop. `-zip` with `.schema` emits
  compatibility DDL, not a ZIP-backed database implementation.
- `-deserialize` replays RedlineDB's saved SQL sidecar when present; it
  does not deserialize a SQLite format-3 image. `-stats` and `.stats` do
  not provide SQLite's statistics report, and `-pcachetrace` has no trace.
  `-vfstrace` prints a compatibility marker on the full path, not real VFS
  events. `-memtrace` does trace allocations.
- `-unsafe-testing` belongs to shell compatibility tests, and
  `-no-rowid-in-view` is accepted without changing view handling. Neither establishes full
  SQLite compatibility. `.auth`, `.expert`, `.lint`, `.scanstats` and
  `.vfsname` have the limitations documented in the full help and parity
  policy.

## Runtime environment

| Key | v5.1.1 behavior |
| --- | --- |
| `REDLINEDB_DEFAULT_DURABILITY` | Unset means Strict for ordinary on-disk opens. Case-insensitive, trimmed `strict`/`full`, `normal`, and `unsafe_dev`/`unsafe-dev`/`off` select the modes. Unknown values panic at default-option construction; release builds abort on panic. |
| `REDLINEDB_QUIET_DURABILITY` | Presence suppresses the notice for an environment-selected non-default durability mode; even value `0` suppresses it. |
| `REDLINEDB_RESULT_DIALECT` | Exact value `postgres` selects the PostgreSQL subset; other values select SQLite. The database reads it at open, and the CLI fixes its dialect at startup. |

Ephemeral `:memory:` databases report `unsafe_dev` durability even when the
on-disk default is Strict. They disappear when the process exits.

```bash doctest
redlinedb -batch :memory: 'PRAGMA redline_durability;'
# prints: unsafe_dev
```

Installer settings such as `VERSION`, `PREFIX`, `REDLINEDB_SHA256`,
`REDLINEDB_VERIFY_ATTESTATION`, `REDLINEDB_ROLLBACK`,
`REDLINEDB_MIGRATE_LEGACY` and `REDLINEDB_LOCK_TIMEOUT` belong to
[installation](install.md). Build, planner and test-runner variables are
developer controls, not a general CLI configuration file. v5.1.1 has no
implemented `REDLINEDB_BENCH_KILL` runtime switch; see
[testing policy](testing.md#budgets-and-stop-conditions).
