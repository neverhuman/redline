# Release notes: kernel (lane kernel)

Draft lines for the v5.0.0 CHANGELOG. The integrator owns `CHANGELOG.md`.

## Testing

- Kernel unit tests now pass when `cargo test` runs them as parallel threads
  of one process, as CI's `cargo test -p redlinedb-kernel` and
  `cargo test --workspace --lib` do. `index::mutate::insert_leaf` counted leaf
  rebuilds through the process-wide `observe` counters, so rebuilds made by
  tests running beside it failed its "direct path rebuilt the leaf" check.
  Test builds now also count each `observe` event on the thread that made it,
  and kernel unit tests read that count. The process-wide counters that
  `redlinedb-bench` reports are unchanged.

## Crash-recovery qualification

- `redlinedb-bench recover` and `recover-matrix` now exit non-zero unless
  every run qualifies, and their reports carry a top-level `passed` field.
  They used to write the report and exit 0 whatever it said, so logs
  recording "exit 0, 24/36 passed" counted as a green lane.
- A new recovery oracle (`redlinedb_bench::recover::oracle`) replaces the
  "recovered rows >= acknowledged rows" count. Children now write
  `key<TAB>sha256(row values)` for each acknowledged transaction. A run
  qualifies only if every acknowledged key comes back in every table its
  transaction wrote, with the exact contents. Apart from those, only the one
  in-flight transaction may appear, and it must be complete. No transaction
  may be half present. Index lookups must match a NOT INDEXED scan (EXPLAIN
  QUERY PLAN confirms both paths), `PRAGMA integrity_check` must be `ok`,
  and a second reopen must see the same image.
- The harness now waits for the child to print READY before it starts the
  kill timer. It counts the fault only when the kill lands on a live child.
  A child that exits before the kill, never reaches READY, or leaves no ack
  log fails the run. On failure the run's temporary directory is kept, and
  the report records its path, the seed, the git SHA and the tail of the
  child's stderr.
- The failpoint matrix now uses the same oracle. A kill case also needs the
  child's panic marker to name the armed failpoint. The in-tree end-to-end
  test used to spawn the libtest binary, which rejected `failpoint-child`
  and "passed" without any failpoint firing. Tests now pass the real binary
  through a hidden `--child-exe` flag.
- `cargo run -p redlinedb-bench -- <subcommand>` works again. The package
  has several binaries and no `default-run`, so the documented
  failpoint-matrix and recovery-matrix lane commands stopped before running
  anything.
- Known red: with the new oracle, `recover-matrix` fails 8 of 36 runs, all
  Redline and all on `PRAGMA integrity_check`. Heap and index pages that
  were never checkpointed stay zero-filled in the page file. Their contents
  are recovered from the WAL, so every acknowledged row reads back exactly,
  but `integrity_check` reads the raw page file and reports "invalid magic".
  A clean shutdown without a checkpoint shows the same errors.

## Database ownership

- An open now takes `owner.lock` before it reads or recovers the database
  image. It used to run full crash recovery first and check the lock
  afterwards, so an open that was about to fail with `Busy` could already
  have truncated the owner's write-ahead log tail or written recovered
  pages. A rejected open now leaves the image untouched.
- Read-only opens take the same exclusive lock whenever
  `process_owner_lock` is on. They used to skip it while still running
  recovery. A read-only open now gets `Busy` while another process has the
  database open, including another read-only open.
- `read_only` now belongs to each `Database` handle. A read-only open in a
  process that already had the database open for writing used to share the
  writable handle's setting and could write.
- The lock uses `File::try_lock`, which is still `flock` on Linux and
  macOS, so it excludes older builds too. The old code skipped the lock on
  every non-Unix platform. On Windows `std` now takes a `LockFileEx` lock
  (not tested in this change), and a platform without file locking fails
  the open with `Unsupported`. A directory where
  `owner.lock` cannot be created (read-only media, no write permission)
  fails with an error that says so.
- Not changed: the CLI `-readonly` flag and the sqlx `mode=ro` attach open
  with `process_owner_lock(false)`, so they still run recovery against a
  database another process may be writing.
- `Database::create` on a directory that holds only a stale `owner.lock`
  creates a fresh database.
- A named ephemeral session (`Database::create_ephemeral`) takes its
  session directory's `owner.lock` before it clears a leftover directory. It
  used to delete the directory first, which could remove a live session of
  the same name in another process sharing the temp root; that now fails
  with `Busy`.

## Directory durability

- A new write-ahead log segment's name is now durable before any record in
  it is acknowledged. Creating the first segment, rotating to the next one,
  and opening a log whose valid end sits exactly on a segment boundary each
  fsync the `wal` directory before a record goes into the segment. The
  open path used to skip that sync, so a Strict commit written to that
  segment could be acknowledged while the file's name was not yet on disk.
- Every open of the log now fsyncs the `wal` directory, not only when it
  creates a segment. A segment name left by a run that died between the
  create and its directory sync is made durable before new records go
  into it.
- Rotation used to switch to the new segment before the directory sync. If
  that sync failed, the log kept writing into a file whose name might not
  survive a power loss. The switch now happens only after the sync
  succeeds. A failed sync fails the append; in the engine it stops the WAL
  writer, so the Strict commit waiting on it returns an error and stays
  invisible.
- Creating the `wal` directory now fsyncs the database root, so the `wal`
  entry is durable too. Before, the root was fsynced once when the page
  file was created, before `wal/` existed.
- A newly created database root is durable in its parent directory, and so
  is each missing ancestor that was created with it. This covers
  `Database::create`, opens with `OpenOptions::create`, and
  `Engine::create`/`Engine::open`. A root that already existed is left to
  whoever created it.
- Directory fsync is part of the kernel's `FileSystem` trait
  (`sync_dir`). On non-Unix targets the standard implementation does
  nothing, so the kernel claims directory-entry durability only on Unix.
  Builds with the `failpoints` feature can fail it through
  `wal::sync_dir`.
- Cost: one extra directory fsync per segment rotation, per log open, and
  per new directory at create time.
- Not changed: the opt-in multi-lane WAL (`wal/lanes.rs`) and the
  feature-gated WAL pipeline (`wal/pipeline.rs`) still create files and
  directories without a directory fsync. Neither is used by the default
  engine. Removing old segments after a checkpoint does not fsync the
  directory either, so after a crash a removed segment can reappear. This
  change does not test how recovery handles one.

## Transaction ids and the WAL position across restarts

- A rolled-back or abandoned transaction logs no abort, so its changes stay
  in the WAL under its id. Recovery now moves the next transaction id past
  every id the scanned WAL names, in record headers and in heap, index and
  commit payloads, whatever the replay start or recovery target. In v4.1.0,
  `INSERT`, `ROLLBACK`, a clean close and a reopen could hand the
  rolled-back id to the next transaction; once that transaction committed,
  even with no writes, the following reopen replayed the rolled-back rows
  as committed. No crash was needed. An earlier kernel fix in this release
  already reserved the ids in record headers; this adds the payload ids,
  the overflow check below and regression tests for rollback, abandoned
  inserts, updates and deletes, and a checkpoint in between.
- A WAL that names the last transaction id (`u64::MAX`) now fails the open
  with `CorruptWal` instead of wrapping the counter back to zero.
- The WAL no longer restarts below the checkpoint recovery replays from.
  When no WAL record survives (the `wal` directory was removed, or a
  checkpoint that ended exactly on a segment boundary pruned the segment
  before it), the log resumes at the next segment boundary at or past the
  checkpoint LSN, the highest page LSN in the page file and the start of
  the highest segment on disk. Before, it restarted at LSN 0; recovery then
  skipped every record written after that reopen, so those commits were
  lost at the next restart. Reading the page LSNs costs one pass over the
  page file, and only on this path.
- A WAL whose records end below the checkpoint while the segment that held
  them is still on disk (truncated or emptied) now fails the open with
  `CorruptWal("wal ends before checkpoint redo lsn")` instead of silently
  restarting below the checkpoint.
- When the log resumes past a highest segment that holds only a torn
  record, that segment is truncated first. Otherwise the torn bytes would
  sit in a segment that is no longer the last, and the next open would
  reject the WAL.
- Not changed: a WAL that reaches the checkpoint is trusted to reach every
  page LSN too; page LSNs are read only when no record survives.

## Page-image redo skips an image the page already holds

- Recovery used to write every heap and B-tree page image after the
  checkpoint over the page, even when the page in the file or in memory was
  newer, which dropped later changes that were on the page. An image now
  replaces a page only when the page's LSN is below the end of the image's
  WAL record. This depends on the WAL never restarting below a page LSN
  (above), so the two ship together.
- An applied image replaces the file copy and any resident copy of the page
  together, and is stamped with its record's end LSN. Before, heap image
  redo wrote only the file, and an older resident copy could later be
  flushed back over it.
- A page-image redo write now makes the next checkpoint sync the page file,
  even if that checkpoint flushes no page. Before, such a checkpoint could
  record a redo LSN past images whose writes were never synced.
- `BtreeIndex::redo_page_image` now takes the record's end LSN, as
  `PageBackedHeap::redo_page_image` already did: a WAL image stores page
  LSN zero, so the image alone cannot say how new it is.

## Checkpoints take a complete cut

- A checkpoint used to write only dirty pages whose LSN was at or below
  the LSN it recorded. A page another writer changed after the checkpoint
  chose that LSN was skipped whole, although it still held older committed
  rows, and the checkpoint then recorded the LSN and pruned the WAL below
  it. Recovery started there and never saw those rows again. A clean close
  lost them too, because closing writes no pages. The second writer did not
  have to commit. With four writers on a 48-page pool this lost rows in
  about one run in six.
- A checkpoint now writes every dirty page, whatever its LSN, after making
  the WAL durable through that page's LSN (under the page's frame lock,
  the order a heap append takes them). The page file is synced before the
  control file is written.
- Heap redo appends a replayed row as a new version and cannot tell whether
  the page file already holds it, so writing heap pages that carry changes
  past the checkpoint LSN would have left a second copy of each such row
  after recovery. A checkpoint therefore writes pages while no logged heap
  change is between its WAL append and its page install (heap appends hold
  a shared gate across that window; the checkpoint holds it exclusively
  while it writes pages, not while it syncs). The control file records the
  WAL position of that instant as a new `heap_redo_lsn`, and recovery
  replays heap records only from there. Index and page-image redo already
  skip what a page holds, so index pages may carry changes past the
  checkpoint LSN.
- The control file format is now version 2. Version 1 files still open
  (their heap redo starts at the checkpoint LSN); an older build refuses a
  version 2 file instead of replaying heap records its page file already
  holds.
- A new control-file generation may not record a checkpoint LSN or heap
  redo LSN below the previous generation's, which may already have pruned
  the WAL below them. Checkpoints were already serialized from the start.
- Recovery to an LSN below the checkpoint's heap redo LSN now fails, as one
  below its checkpoint LSN did; the page file already holds heap changes up
  to the heap redo LSN.
- Cost, measured with a release-build probe (4 writers inserting 200-byte
  rows in Normal mode, a checkpoint every 100 ms, 4096-page pool): on NVMe,
  about 48,000 to 50,000 rows/s against 55,000 to 56,000 before, with
  checkpoint times unchanged (median about 85 to 100 ms) and the worst
  single insert-and-commit 39 to 108 ms against 32 to 94 ms before; on
  tmpfs, 59,000 to 61,000 rows/s against 60,000 to 65,000. A single
  writer's checkpoint of 172 pages takes the same time as before.
- Not changed: a crash in the middle of a checkpoint's page writes, before
  its control file lands, can still leave a second copy of a heap row
  written after the previous checkpoint on a page that existed then.
  Recovery replays that row again from the previous generation. Reads
  through the row directory see one version; a page scan can see both.
  Closing this needs heap WAL records that name their page, or a
  double-write area for checkpoint pages.
