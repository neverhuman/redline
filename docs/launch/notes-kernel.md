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
  A clean shutdown without a checkpoint shows the same errors. Resolved
  below ("integrity_check passes pages the file never received"): on the
  lane head with that change, `recover-matrix --config
  crates/bench/bench/recovery-matrix.toml --seed 7` passes 36 of 36 runs
  and exits 0.

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

## Recovery writes each replayed heap row once

- Heap redo puts each replayed row into a new version on a page it
  allocates past the end of the page file, stamped LSN zero, and eviction
  may write that page before recovery takes its closing checkpoint. A
  process that died during recovery after such a write, or after a
  recovery small enough to take no checkpoint once eviction had written a
  replayed page, left those rows in the page file, and the next recovery
  replayed them onto new pages again: a page scan returned each row two or
  three times.
- Recovery now empties every heap and undo page past the checkpoint's page
  count (all of them when there is no checkpoint) before heap redo, and
  queues them for reuse. A checkpoint writes every page that exists when
  it writes its heap pages, so a heap page past its page count holds only
  replay output or rows whose records recovery replays anyway. Index pages
  are left alone; their redo already skips what a page holds. A torn page
  there is left as it was, because its kind cannot be read.
- Builds with the `failpoints` feature can stop recovery once heap redo is
  done through `engine::recovery::after_heap_replay`.

## A reinserted row links to the version it replaces

- Inserting a row id that already has a version (a delete, then an insert
  of the same row id) appended the new version with no undo link. Two
  things went wrong. A snapshot taken before the delete saw the row vanish
  once the reinsert committed, because the new version's undo chain was
  empty. And when a transaction deleted and reinserted a row and the new
  version landed on a reused page with a lower id than the tombstone, a
  reopen that rebuilt the row directory from the pages picked the
  tombstone, so the row was gone.
- The reinsert now links to a before-image of the version it replaces, as
  an update does. Older snapshots reach the tombstone and the original row
  through it, and the directory rebuild follows the link.
- Not changed: files written before this change can still hold an
  unlinked delete and reinsert of one transaction; for those the rebuild
  still picks by page position.

## A B-tree split is all or nothing

- A leaf split installed the two leaf halves first and only then pinned
  the parent and allocated a page for a parent split or a new root. When
  that allocation or pin failed (a pool with no frame to give), the leaf
  was left with a right sibling its parent never named, or a root leaf had
  a sibling and no root above it. The next split under it took the leaf
  for its parent and failed with "expected internal page", which a
  24-frame stress run hit.
- A split now pins, reads and allocates every page it will change (both
  leaves, each parent it overflows, a new root and the meta page) and
  stages their new images before it appends the first WAL record or
  installs a page. A failure there leaves the tree as it was and the
  insert returns the error; the pages it had allocated stay behind as
  empty, unreferenced pages. Only the WAL append itself can still fail
  mid-split, and a WAL failure stops the engine.
- A descent that moves right to a sibling now records that sibling in
  place of the page it left, so the parent chain a split walks holds one
  page per level. It used to hold both, and a split would take the left
  sibling for the parent.

## In-memory databases grow past their buffer pool

- An in-memory database (`:memory:`, `Database::create_in_memory`,
  ephemeral sessions) runs on a volatile kernel engine: no WAL, no
  recovery, nothing to checkpoint. Its pages kept the placeholder LSN a
  logged change stamps, and eviction writes a dirty page on its own only
  when that LSN is zero, so once the database outgrew its pool (256 pages
  by default) every insert failed with "no unpinned frame available for
  eviction".
- A volatile engine's buffer pool now treats its page file as scratch
  space: eviction may write any unpinned dirty page there and read it back
  later. Memory stays bounded by the pool; the scratch file grows instead.
  Persistent engines are unchanged.

## integrity_check passes pages the file never received

- `PRAGMA integrity_check` read every page of the page file and reported
  "invalid magic" for a zero-filled one. A page reaches the file only when
  a checkpoint or eviction writes it, and a higher page can get there
  first: a page allocated while a checkpoint took its snapshot, or one a
  crash cut off before any write, whose rows recovery replays onto new
  pages. Such a page holds nothing the database refers to, yet the check
  failed, which is the known-red `recover-matrix` result above and what a
  clean close without a checkpoint showed.
- A page whose bytes are all zero now counts as never written, as the full
  check (`redline_full_check`) already skipped it. Any other unreadable
  page, a checksum failure included, is still reported.

## Pressure checkpoints are on for SQL databases

- A persistent database whose dirty pages outgrew the buffer pool failed
  the write with "no unpinned frame available for eviction": eviction
  writes a logged page only as part of a checkpoint, and SQL, CLI and FFI
  databases never asked for one on their own. Pressure checkpoints had
  been made opt-in (single writer only) because of the incomplete cut
  fixed above.
- `redlinedb-sql` now enables them for every persistent database it
  creates, or opens to the end of its WAL (`Engine::
  enable_pool_pressure_checkpoints`). Opens to an earlier recovery target
  and in-memory databases do not use them. The kernel itself still leaves
  them to its caller.
- With many writers, the frames one checkpoint cleans can be dirtied again
  before the thread that asked for it gets to evict one. An allocation now
  asks for up to eight checkpoints before it fails, and a writer that
  waited on another thread's checkpoint takes none of its own.
- Only a pool whose frames are all pinned still fails the allocation.

## Recovery checks the WAL and the checkpoint it starts from

- Recovery now reads the control files, the chosen generation's
  transaction status and the catalog, and checks the scanned WAL against
  them, before it changes any file (workplan R6, R9 steps 1-2). A check
  that fails leaves every file as it was.
- A length or header damaged inside the final WAL segment was taken for
  a torn tail: recovery dropped the committed records after it and
  truncated the segment there. The scan now looks past a tail for a whole,
  checksum-valid record at its own position. Finding one fails the open
  with "valid wal record after torn tail". `WalReader::
  salvage_after_torn_tail(true)` reports the record's offset instead, for
  inspecting such a log; the engine has no salvage mode.
- The WAL must now hold every record recovery replays. With no checkpoint
  it must start at LSN 0 ("wal does not start at lsn 0 and no checkpoint
  covers the records before it"); with one, it must reach back to the
  checkpoint LSN ("wal starts after the checkpoint redo lsn"). A segment
  missing after the first record's segment fails the open even when no
  later record crosses the gap. A lost WAL beside a page past LSN 0, with
  no control file, fails too. Each of these used to replay what was left
  and lose the rest silently.
- A checkpoint now prunes the WAL only below the previous generation's
  checkpoint LSN, which the other control slot still names, so falling
  back to that slot finds its WAL. Pruning used to follow the newest
  generation, and a fallback then lost every commit between the two. The
  WAL kept on disk grows to about two checkpoints' worth. Transaction
  status files older than the previous generation are deleted; they used
  to accumulate one per checkpoint.
- A control slot that does not decode beside a missing one used to mean
  "never checkpointed": recovery replayed a pruned WAL from LSN 0. With no
  valid control file, recovery now replays the whole WAL only if it starts
  at LSN 0, and fails with "no valid control file and the wal does not
  start at lsn 0" otherwise. A valid slot beside a corrupt one needs a
  WAL that covers it, or the open fails with "fallback checkpoint lacks
  required WAL".
- When the newest valid generation cannot be used (its transaction status
  is missing or its WAL check fails), recovery falls back to the other
  slot and starts heap redo at the newer generation's heap redo LSN, since
  the page file holds that generation's heap. `RecoveryReport` gains
  `skipped_generation` and `warnings`, which name corrupt slots and any
  generation skipped.
- A schema file that does not decode, or is missing once a checkpoint
  exists or the page file holds pages, now fails the open unless the WAL
  holds a catalog snapshot. It used to load the empty bootstrap schema,
  which dropped every table.
- Open: a checkpoint that dies after it writes its pages and before its
  control file is durable (or whose control write tears) leaves the page
  file ahead of the generation recovery starts from. Heap redo then adds a
  second copy of each row that checkpoint wrote to a page the older
  generation already had: reads by row id are right, page scans return the
  row twice. This was so before these changes. Closing it needs the
  checkpoint's page state recorded before its page writes (or heap redo
  that skips what a page already holds).

## Uncertain commits, and COMMIT returns only once it is visible

- A commit whose WAL write or fsync failed after its commit record was
  queued returned a plain error and was marked aborted in memory. The
  record could still be in the file, and the next open then recovered the
  transaction as committed, so a caller that took the error as a rollback
  and retried could apply a change twice. Such a commit now fails with
  the new kernel error `CommitOutcomeUnknown { tx_id, end_lsn, cause }`.
  SQL reports it as the existing `CommitMaybeCommitted` ("commit outcome
  uncertain"), which the `redlinedb` crate maps to `ErrorCode::IoErr` and
  the C API to `RLDB_IOERR`. An error before the record is queued is still
  a plain error, and that transaction never comes back. The transaction is
  not visible in the process either way.
- A commit that the writer made durable and then failed on a later batch,
  before the committer woke, was reported failed and aborted in memory
  although it was on disk. `flush_until` and `write_until` now check the
  durable (or written) LSN before the failure.
- The WAL writer kept only the text "wal writer failed". It now keeps the
  step (`write`, `flush` or `rotate`), the I/O error kind and the LSN, and
  every later append or wait fails with the new `Error::WalWriterFailed`,
  whose message still starts "wal writer failed". The `redlinedb` crate
  and the C API report it as an I/O error.
- A CSN reserved by a commit that then failed before queueing its record
  stayed pending forever, so no later commit became visible to a new
  snapshot. The reservation is now a guard that gives the CSN up when the
  commit fails or panics, and recovery settles every CSN below the next one
  once replay ends, so a CSN the WAL lacks cannot hide the commits after
  it.
- `COMMIT` returned as soon as its own CSN published, even when an earlier
  CSN was still between its barrier and its publish. A snapshot begun after
  `COMMIT` returned, including the same connection's next statement, could
  then miss that commit. The commit now releases its locks and the
  checkpoint horizon, then waits until the published CSN reaches its own
  (`ConcurrentTxStatus::wait_published`), and warns on stderr after 5 s.
  Existing snapshots are unchanged. Schema and index handles still publish
  with the transaction, not with the CSN frontier, so a statement that
  starts during that wait can see a new table before its rows.
- Cost, measured with a release-build probe (one-row insert-and-commit
  loops on ext4 NVMe on a shared host, two rounds of three repetitions per
  build): one Normal writer, 57,700 to 65,100 commits/s against 40,600 to
  66,200 before (median latency 14 to 17 us against 14 to 19 us); 16
  Normal writers, 42,900 to 51,500 commits/s against 30,500 to 58,200,
  median latency 277 to 349 us against 243 to 384 us (the middle run about
  10% higher), p99 758 to 840 us against 740 to 2,900 us; 16 UnsafeDev
  writers, 110,500 to 126,500 against 110,700 to 130,700. Strict is bound
  by a 10 ms fsync on this disk and showed no difference beyond noise.
- Not changed: `CommitMaybeCommitted` carries no transaction id or LSN at
  the SQL layer; the kernel error does. After a WAL failure the database
  stays open for reads, and writes keep failing until it is reopened.
