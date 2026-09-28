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
