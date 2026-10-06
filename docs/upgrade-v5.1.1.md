# Upgrading from v5.1.0 to v5.1.1

v5.1.1 fixes automatic rowid allocation that could overwrite a live row in a
different table. Upgrade before relying on automatic keys. The database
format, index-format epoch and C ABI major version stay the same; v5.1.0
directories open without an index migration.

## Before changing the binary

Stop your application's writer cleanly, or take a physical backup through its
existing `Database` handle. The separate CLI needs exclusive ownership of the
directory, so do not run it against a database another process has open.

With the **v5.1.0** binary and a destination that does not exist:

```bash upgrade
old_redlinedb=/path/to/v5.1.0/redlinedb
database=/path/to/app.redline
backup=/path/to/app-before-v5.1.1.redline
"$old_redlinedb" --build-info --json
"$old_redlinedb" backup "$database" "$backup" --physical
```

Keep that backup and its binary. An installer rollback changes executable
links; it does not restore a database or undo changes made after the backup.
Check application row counts and keys as well as `PRAGMA integrity_check`.
Integrity checking cannot recover a row that the older rowid bug already
overwrote. Recover missing data from a backup or the application's records.

## Install and verify

Use the pinned installer in [the installation guide](install.md). Then verify
the executable that the application will actually run:

```bash doctest
redlinedb --version
# prints: redlinedb v5.1.1 (tested against SQLite 3.53.1)
```

`redlinedb --build-info --json` must name tag `v5.1.1` and source commit
`9277455d5ad008252053a81d18add39b8cdc8f7b`. Reopen the directory, check the
application's rows and indexes, and run `PRAGMA integrity_check` before
resuming writes. Keep the default Strict durability for on-disk data that
must survive a process crash. No power-loss certification is claimed.

## Automatic keys change

Each table now allocates its own rowids. A new table's first automatic key is
1 even when another table already has rows. An existing table continues from
its own highest live key. Explicit keys are preserved. Do not use
automatically assigned keys as globally unique identifiers across tables.
`last_insert_rowid()` reports the key assigned by that connection's insert.

```sql doctest
CREATE TABLE a(id INTEGER PRIMARY KEY, value TEXT);
CREATE TABLE b(id INTEGER PRIMARY KEY, value TEXT);
INSERT INTO a VALUES(7, 'existing');
INSERT INTO b VALUES(20, 'other table');
INSERT INTO a VALUES(NULL, 'new');
SELECT id, value FROM a ORDER BY id;
-- prints: 7|existing
-- prints: 8|new
```

The fix also protects restored keys after rollback and serializes conflicting
rowid writes and schema changes. Applications must still handle transaction
errors and uncertain commit outcomes as described in
[the transaction guide](manual/07-transactions.md).

`AUTOINCREMENT` remains limited: `sqlite_sequence` is not persisted across a
reopen. A deleted high key can therefore be reused after reopening. The
ordinary allocation fix does not establish SQLite's lifetime no-reuse
guarantee. See [known limitations](known-limitations.md).

## Embedded applications

The Rust API remains experimental. Pin `tag = "v5.1.1"`, retain the
application's `Cargo.lock`, rebuild, and run its integration tests. C
consumers should use the headers and native library from the same release;
the implemented SQLite-shaped subset is described in
[API stability](api-stability.md) and [security capabilities](security-capabilities.md).

The documentation check opens a physical backup and a database made by the
published v5.1.0 binary with the published v5.1.1 binary. This checks the
documented upgrade example, not every application schema or an arbitrary
downgrade path.
