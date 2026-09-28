//! `Engine::parallel_scan_relation` returns what the serial read returns
//! under the same snapshot while other transactions keep committing.
//!
//! The scan reads rows from a snapshot of the row directory taken when it
//! starts. Writers keep moving rows' newest tuples to new pages, deleting
//! rows and adding rows while it runs, and a checkpoint flushes pages in the
//! middle. None of that may make the scan skip a row the snapshot sees,
//! return one twice, or return a version the snapshot does not see.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use redlinedb_kernel::engine::{CommitDurability, Engine, EngineConfig, Txn};
use redlinedb_kernel::format::{RelId, RowId};
use redlinedb_kernel::txn::Isolation;
use redlinedb_kernel::wal::WalConfig;
use redlinedb_kernel::{Error, Result};
use tempfile::TempDir;

const REL: RelId = RelId(1);
const WRITERS: u64 = 3;
const ROWS_PER_WRITER: u64 = 80;
/// Bounds the pages the writers dirty, so the pool never fills.
const ROUNDS_PER_WRITER: u64 = 40;
const SCANS: usize = 40;

fn config() -> EngineConfig {
    EngineConfig {
        rel_id: REL,
        commit_durability: CommitDurability::Strict,
        busy_timeout: Duration::from_millis(2_000),
        heap_lanes: 4,
        page_size: 4096,
        buffer_pool_pages: 4096,
        wal: WalConfig {
            group_commit_delay_us: 0,
            ..WalConfig::default()
        },
        ..EngineConfig::default()
    }
}

/// A 200-byte payload tagged with the writer, the row's key and a version.
fn payload(writer: u64, key: u64, version: u64) -> Vec<u8> {
    let mut bytes = vec![(key % 251) as u8; 200];
    bytes[..8].copy_from_slice(&writer.to_le_bytes());
    bytes[8..16].copy_from_slice(&key.to_le_bytes());
    bytes[16..24].copy_from_slice(&version.to_le_bytes());
    bytes
}

type Model = BTreeMap<RowId, Vec<u8>>;

fn serial_read(engine: &Engine, tx: &mut Txn) -> Model {
    let mut out = Model::new();
    for row in engine.relation_rowids(REL).unwrap() {
        if let Some(bytes) = engine.get_for_relation(tx, REL, row).unwrap() {
            out.insert(row, bytes);
        }
    }
    out
}

fn relation_scan(engine: &Engine, tx: &mut Txn, workers: usize) -> Model {
    let mut out = Model::new();
    for row in engine.parallel_scan_relation(tx, REL, workers).unwrap() {
        assert_eq!(row.rel_id, REL);
        let row_id = row.row_id;
        assert!(
            out.insert(row_id, row.payload).is_none(),
            "{workers}-worker relation scan returned {row_id:?} twice"
        );
    }
    out
}

/// One round in one transaction: a new version of every row in `rows`, the
/// row at `victim` deleted, and a row with `key` inserted. Returns the new
/// row's id.
fn write_round(
    engine: &Engine,
    writer: u64,
    rows: &[(u64, RowId)],
    version: u64,
    victim: usize,
    key: u64,
) -> Result<RowId> {
    let mut tx = engine.begin(Isolation::Snapshot)?;
    let mut changes = || -> Result<RowId> {
        for (key, row) in rows {
            engine.update(&mut tx, *row, payload(writer, *key, version))?;
        }
        engine.delete(&mut tx, rows[victim].1)?;
        engine.insert(&mut tx, payload(writer, key, version))
    };
    match changes() {
        Ok(row) => {
            engine.commit(tx)?;
            Ok(row)
        }
        Err(err) => {
            engine.rollback(tx)?;
            Err(err)
        }
    }
}

/// One writer's rounds, until `stop` or `ROUNDS_PER_WRITER` rounds.
fn write_rounds(engine: &Engine, writer: u64, stop: &AtomicBool, rounds: &AtomicU64) {
    let mut rows: Vec<(u64, RowId)> = Vec::new();
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    for key in 0..ROWS_PER_WRITER {
        let row = engine.insert(&mut tx, payload(writer, key, 0)).unwrap();
        rows.push((key, row));
    }
    engine.commit(tx).unwrap();

    let mut version = 0;
    let mut next_key = ROWS_PER_WRITER;
    while !stop.load(Ordering::Acquire) && version < ROUNDS_PER_WRITER {
        let victim = (version as usize) % rows.len();
        match write_round(engine, writer, &rows, version + 1, victim, next_key) {
            Ok(row) => {
                version += 1;
                rows.remove(victim);
                rows.push((next_key, row));
                next_key += 1;
                rounds.fetch_add(1, Ordering::Relaxed);
            }
            // Another writer's commit can still hold back the snapshot a
            // new transaction takes, so this writer's last commit may not
            // be visible yet. A client retries the round.
            Err(Error::SerializationFailure) => thread::yield_now(),
            Err(err) => panic!("writer {writer}: {err:?}"),
        }
    }
}

#[test]
fn relation_scan_under_concurrent_writers_matches_the_serial_read() {
    let dir = TempDir::new().unwrap();
    let engine = Engine::create(dir.path(), config()).unwrap();
    let stop = AtomicBool::new(false);
    let rounds = AtomicU64::new(0);

    thread::scope(|scope| {
        for writer in 0..WRITERS {
            let (engine, stop, rounds) = (&engine, &stop, &rounds);
            scope.spawn(move || write_rounds(engine, writer, stop, rounds));
        }
        let deadline = Instant::now() + Duration::from_secs(60);
        while rounds.load(Ordering::Relaxed) < WRITERS {
            assert!(Instant::now() < deadline, "the writers made no progress");
            thread::yield_now();
        }
        for scan in 0..SCANS {
            let mut tx = engine.begin(Isolation::Snapshot).unwrap();
            let workers = if scan % 2 == 0 { 4 } else { 1 };
            let scanned = relation_scan(&engine, &mut tx, workers);
            if scan % 8 == 3 {
                engine.checkpoint().unwrap();
            }
            let serial = serial_read(&engine, &mut tx);
            assert!(
                scanned == serial,
                "scan {scan}: {workers}-worker relation scan has {} rows, the serial read {}",
                scanned.len(),
                serial.len()
            );
            engine.rollback(tx).unwrap();
        }
        stop.store(true, Ordering::Release);
    });
    assert!(
        rounds.load(Ordering::Relaxed) > WRITERS,
        "the writers must commit while the scans run"
    );
}
