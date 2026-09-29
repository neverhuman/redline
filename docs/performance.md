# Performance Build Profiles

Three cargo profiles are available for the `redlinedb` binary, in order
of increasing perf and decreasing portability.

| Profile | When | How |
|---|---|---|
| `release` | Default, what we ship | `cargo build --release` |
| `release-native` | Local benches on the build host | `RUSTFLAGS="-C target-cpu=native" cargo build --profile release-native -p redlinedb-cli` |
| `release-pgo` | Reproducible bench numbers for PR descriptions | `scripts/perf/pgo.sh` (two-pass: instrument → train → recompile) |

All three inherit `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`,
`panic = "abort"`, and stripped symbols from `release`.

The checked-in Linux x86_64 default in `.cargo/config.toml` is
`target-cpu=x86-64-v3` for portability. The perf scripts intentionally
override that with `target-cpu=native` through
`scripts/perf/lib-rustflags.sh`; binaries produced by those scripts are
benchmark artifacts for the build host, not portable release artifacts.

## `release-native`

Inherits `release`. The `target-cpu=native` rustflag must be supplied via
env because Cargo does not support per-profile rustflags on stable. Use
only when the binary will run on the same CPU it was built on.

## `release-pgo`

Inherits `release-native`. Profile-guided optimization trains on the
parity workload via the `redline-testing` runner. Two passes -
instrumented build, training run, final build - are managed by
`scripts/perf/pgo.sh`.

`scripts/perf/pgo.sh` accepts `REDLINE_CARGO_FEATURE_ARGS` for local
allocator A/B runs:

```bash
REDLINE_CARGO_FEATURE_ARGS="--no-default-features --features alloc-jemalloc" \
  scripts/perf/pgo.sh
```

PGO always trains against the complete official corpus through the verified
external `redline-testing` runner. The former core-owned quick/medium replay
paths were retired because they duplicated the official evidence producer.

By default the script also allocates unique temporary PGO data/profile
directories per run, which avoids collisions when multiple perf jobs are
active. Override `PGO_DATA_DIR` and `PGO_PROFILE_DIR` if you want to
pin the output locations.

## W2 Matrix

Use `scripts/perf/w2-matrix.sh` for repeatable W2 build/profile runs. It
builds selected profile/allocator variants, copies each binary under
`target/perf/w2-matrix/<run-id>/bin/`, optionally runs a perf lane, and
writes one manifest row per variant to `manifest.jsonl`.

The default matrix is bounded:

```bash
just perf-w2-matrix
```

Heavier runs can opt into PGO and BOLT explicitly:

```bash
scripts/perf/w2-matrix.sh \
  --suite full \
  --profiles release,release-native,release-pgo,release-pgo-bolt \
  --allocators mimalloc,jemalloc
```

The matrix passes the allocator feature set through to `pgo.sh`, so the
`release-pgo` and `release-pgo-bolt` legs train and rebuild under the
selected allocator as well.

The matrix accepts `--suite none` for build-only comparisons and `--suite
full` for an external-corpus measurement. It does not expose local subset
producers.

Allocator choices are currently the mutually exclusive CLI features
`alloc-mimalloc`, `alloc-jemalloc`, and `alloc-snmalloc`. There is no
system-allocator feature yet, so the W2 matrix does not claim a system
allocator leg.

## Per-binary builds

For the CLI binary specifically:

```bash
RUSTFLAGS="-C target-cpu=native" \
  cargo build --profile release-native -p redlinedb-cli --bin redlinedb
```

Output lands at `target/release-native/redlinedb` (the directory name matches the profile, not `release/`).

## Engine throughput scoreboard

The README's shell table times whole `redlinedb` and `sqlite3` processes on
the parity corpus's small scripts, so process start-up dominates it. The
engine scoreboard measures the engines themselves: `redline-scoreboard`
(`crates/scoreboard`) links RedlineDB through its Rust API and SQLite
through rusqlite, and runs the same fixed work on both in one process.

- **Workloads.** `redline-scoreboard list` prints the catalog: writes
  (autocommit, batched and bulk inserts, updates, deletes), point reads
  (prepared, and a new SQL string each time), rowid ranges, secondary-index
  reads, scans, `GROUP BY`, top-k, joins and reopening a database. Each
  does a fixed amount of work, never "as much as fits in a time budget", so
  a faster engine cannot change what a later measurement sees.
- **Isolation.** Each engine's database is built once per scale by one
  seeded generator ("image"). Every repetition runs in its own process on a
  fresh copy of the image, after an untimed warm-up read. The run refuses to
  finish if an image changed.
- **Correctness.** Every record carries a digest of what the workload read
  or left behind. The summary refuses a bundle in which the engines, or two
  versions, disagree: a fast wrong answer is not a result.
- **Pairs.** `normal` pairs RedlineDB `Normal` durability with SQLite WAL
  and `synchronous=NORMAL`, on tmpfs. `strict` pairs `Strict` with
  `synchronous=FULL` on a real disk, for the writing workloads only.
  - Both engines get a 64 MiB page cache.
  - SQLite uses 4 KiB pages, no mmap, in-memory temp storage, foreign keys
    off (RedlineDB's default), and `locking_mode=EXCLUSIVE`. RedlineDB holds
    an exclusive owner lock on its database, so SQLite is not made to take a
    file lock per statement.
  - Every query runs on the calling thread. RedlineDB's own WAL-writer and
    prefetch threads run as they always do.
  - Each case reads the settings back and refuses to run if one differs. For
    an open workload the check comes after the clock stops.
- **Reopening.** `open_after_updates` starts from an image whose 5,000
  updates are still in the log for both engines:
  - RedlineDB does not checkpoint when a database closes.
  - SQLite's automatic checkpoint is off while the image is built, and the
    harness leaves its connection unclosed, because SQLite checkpoints when
    the last connection closes.
- **Timing.** Resource figures (CPU time, I/O, RedlineDB's work counters)
  cover exactly the timed work, not statement preparation or the digest
  query. Copied images are synced before timing.
- **Build.** The harness is its own crate because `redlinedb-bench` turns
  the kernel's failpoints on. Build it with `-p redlinedb-scoreboard` only.
  `scripts/perf/build-scoreboard.sh <ref> <label>` builds the harness of
  one commit against another version's engine in a removed sandbox clone.
  It refuses failpoint or debug builds, and any `Cargo.lock` change beyond
  the harness package.

A bundle compares versions on one host:

```bash
scripts/perf/build-scoreboard.sh v5.1.0 v5.1.0
scripts/perf/build-scoreboard.sh HEAD candidate
scripts/perf/scoreboard-bench.sh --bundle <name> --runs 3 --pairs normal,strict \
  v5.1.0=target/scoreboard/v5.1.0/redline-scoreboard \
  candidate=target/scoreboard/candidate/redline-scoreboard
target/scoreboard/candidate/redline-scoreboard render \
  --bundle benchmark-results/perf/releases/<name> --target README.md
```

`scoreboard-bench.sh` pins every version to the same CPUs, rotates their
order from run to run, and waits for a quiet host: no CI job and low load.
SQLite runs beside each version and serves as the control group.

`redline-scoreboard summarize` blocks publication when any of these holds:
- fewer than three runs;
- any version, or the SQLite beside it, missing a workload of a pair in any
  run;
- a failed or timed-out repetition;
- a record at another scale;
- reduced work (`--work-divisor` above 1);
- a series that ran more than one engine version;
- a result mismatch;
- a run that started on a busy host, or saw a CI job at any time (sampled
  every 10 seconds);
- the normal pair not on tmpfs;
- SQLite beside different versions differing by more than 10%. Open times,
  a few milliseconds each, are exempt from this last check.

A speedup is beyond noise only when both of these hold:
- the two versions' run ranges do not overlap;
- it moves by more than 5%, or by more than SQLite's own spread across the
  newer version's runs, whichever is larger.

Anything smaller is marked as within noise. `scoreboard-bench.sh` refuses a binary that carries failpoints or
debug assertions, or that is not the one its `build.json` describes under
the label given. The README block between `<!-- engine-throughput:begin -->` and
`<!-- engine-throughput:end -->` is generated from `summary.json`. The
`redlinedb-scoreboard` library tests fail when it no longer matches its
bundle's raw records.
