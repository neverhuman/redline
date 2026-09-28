# Durability contract

This page says which failures a commit survives in each durability mode, which of those statements a test receipt backs, and what the engine does not promise. Chapter [Transactions and durability](07-transactions.md) explains the modes themselves. Where this page and another page disagree, this page is the one that was checked against a receipt.

A durability statement here is a *claim* only when it carries a claim tag, an HTML comment of the form `claim:durability.<mode>.<failure-model>`. The release gate `ops/ci/durability-claim-gate.sh` refuses to publish a release while any tagged claim lacks a passing receipt for that release's source (see [Receipts](#receipts)). Everything else on this page is design intent or a known limit, and says so.

## Modes and failure models

"Survives" means: every transaction whose `COMMIT` returned success comes back after the failure, with its exact contents, and no transaction comes back half applied.

| Mode | Process kill (SIGKILL, panic, OOM kill) | OS crash or power loss | Media corruption |
| --- | --- | --- | --- |
| `Strict` (default) | **Survives**, on the filesystem, mount options and kernel a passing receipt names. <!-- claim:durability.strict.process-kill --> | **Not claimed.** `COMMIT` returns after the log record is fsynced and new log segments are fsynced into their directory, which is what power-loss safety needs from the engine. No LazyFS or VM-reset receipt exists, and the result also depends on the device honouring fsync. | **Detected, not repaired.** |
| `Normal` | Expected to survive: `COMMIT` returns after `write(2)` hands the record to the operating system, which keeps it when only the process dies. No receipt in this release, so this is not a claim. | **Not survived.** Commits that returned can be lost; the schema file is written without fsync too. | **Detected, not repaired.** |
| `UnsafeDev` | **Not survived.** `COMMIT` returns once the record is queued, before the operating system has the bytes, and close skips the final flush. | **Not survived.** | **Detected, not repaired.** |

Media corruption means bytes that change on the device after they were written. Every page carries a CRC32 that is checked each time the page is read, and every log record carries its own checksum. A damaged page fails the read that touches it and is listed by `PRAGMA integrity_check`. A damaged record inside the log fails the open. Nothing rebuilds the lost bytes: restore from a backup (chapter [Files and day-to-day operation](09-operate.md)).

## What recovery does with an uncertain commit

A `COMMIT` that fails after its record was queued returns `CommitMaybeCommitted` (`ErrorCode::IoErr` in the `redlinedb` crate, `RLDB_IOERR` in the C API). The transaction is not visible in that process, and every later write fails until the database is reopened. After the reopen the transaction is there exactly when its record reached the log file whole. Check for it before you retry anything that must happen once. Chapter [Transactions and durability](07-transactions.md#when-commit-fails) has the details.

A transaction whose record is whole in the log comes back after a crash even when its `COMMIT` never returned. The receipt counts that one in-flight transaction as allowed, not as a failure.

## Visibility

In every mode, a commit is visible only after it has passed that mode's barrier, and `COMMIT` returns only once a transaction that begins afterwards sees it. So on `Strict`, anything another transaction could read survives a process kill as well. Snapshots that were already open do not change.

## One process owns a database

An open takes `owner.lock` in the database directory, exclusively and without waiting, before it reads the image or runs recovery. A second process gets `ErrorCode::Busy` and has not changed any file. A read-only open takes the same lock and still runs crash recovery, which can rewrite the log tail. The shell's `-readonly` flag and the sqlx `mode=ro` URL open without the lock: point them only at a database no process has open. None of the durability statements on this page hold for two processes writing one directory.

## Checkpoints and log retention

A checkpoint writes every dirty page, syncs the page file, then writes one of two control slots. It removes log segments only below the checkpoint the other slot names, so a recovery that falls back to the older slot still finds its log. The log on disk grows to about two checkpoints' worth. Removing a segment does not fsync the `wal` directory, so after an OS crash a removed segment can reappear; recovery of that case is not tested.

A process killed while a checkpoint is writing pages, before its control slot lands, can leave a second copy of a row written since the previous checkpoint. Reads by row id return one version. A page scan can return that row twice. This is a known open defect, not covered by the receipt's workload, which forces no checkpoint.

## Torn log tails

A crash during a log write can leave the start of a record at the end of the log. Recovery does not replay it and changes no log byte until it has succeeded. It then copies the torn bytes to `wal/salvage/<segment>-<offset>.torn`, syncs the copy, and only then cuts them from the segment. A damaged record followed by a valid one is not a torn tail: the open fails instead of dropping the later commits. `PRAGMA redline_recovery_report` names what the last recovery found. Salvage files are never read again.

## Limits of the tested configuration

- **tmpfs and ramfs.** fsync does nothing there, and the files are gone after a reboot. `Strict` on tmpfs survives a process kill for the same reason `Normal` does, and nothing more. The receipt tool refuses to take a `Strict` receipt on tmpfs or ramfs.
- **`wal_pipeline`.** The WAL writer thread behind the `wal_pipeline` cargo feature is off in default and release builds. It creates files without a directory fsync, recovery is not tested against it, and no receipt covers it. The throughput numbers quoted for it in the README say nothing about durability.
- **Other cargo features.** `wal_cross_lane_coalescer`, `numa` and `failpoints` are not in release builds and no receipt covers them. The receipt tool rejects a binary built with kernel failpoints.
- **Platforms.** Directory fsync is implemented on Unix only. On other platforms the engine claims nothing about directory entries.

## Receipts

`redlinedb-bench durability-evidence` produces a receipt for the shipped `redlinedb` shell, not for a test binary. Build that shell on its own; building it together with `redlinedb-bench` turns kernel failpoints on, and the tool rejects such a binary.

```sh
cargo build --release --locked -p redlinedb-cli
cargo run --release --locked -p redlinedb-bench --bin redlinedb-bench -- \
  durability-evidence --binary target/release/redlinedb \
  --mode strict --failure-model process-kill \
  --work-dir /path/on/the/disk/you/care/about \
  --out durability-evidence.json
```

Each scenario feeds the shell one two-row transaction per key and prints an acknowledgement after each `COMMIT` returns. The harness writes every acknowledgement to a ledger it fsyncs, sends SIGKILL at a seeded point, then opens the database twice more with the same binary. A scenario passes when every acknowledged transaction is present with its exact typed values, nothing unacknowledged beyond the one in-flight transaction is present, no transaction is half present, an index probe agrees with a full scan, `PRAGMA integrity_check` is `ok`, `PRAGMA redline_durability` reports the requested mode, and the second recovery reads back the same image as the first. The command exits non-zero unless every scenario passes.

The receipt records the source commit and whether the tree was dirty, the binary's SHA-256, the failpoint scan, the toolchain and target, the filesystem type and mount options, the block device, the kernel, the seeds, the scenario counts, the SHA-256 of every raw log, and a `limitations` list. A SIGKILL is not a power cut: the operating system keeps every byte the process wrote, so a process-kill receipt says nothing about the page cache or the device cache.

`redlinedb-bench durability-evidence-verify --receipt <file> --claim durability.strict.process-kill` checks one receipt against one claim. The release gate runs it for every claim tag in `README.md` and `docs/`, over the receipts in `benchmark-results/durability/`. A receipt backs a claim only when it passed every one of at least 10 scenarios, was taken from a clean tree whose commit is an ancestor of the release commit with no change to `crates/`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` or `.cargo/` in between, came from a binary without failpoints, read back the requested mode, and was not taken on tmpfs for `Strict`.
