#![cfg(feature = "failpoints")]
//! A process that dies during recovery must not leave a second copy of a
//! replayed row behind.
//!
//! Heap replay writes each row into a new version on a new page, stamped LSN
//! zero, and eviction may write that page before recovery checkpoints. The
//! `engine::recovery::after_heap_replay` failpoint panics once heap redo is
//! done, after eviction has written such pages; the next open replays the
//! same records again.

use std::collections::BTreeMap;
use std::time::Duration;

use redlinedb_kernel::engine::{CommitDurability, Engine, EngineConfig};
use redlinedb_kernel::failpoints;
use redlinedb_kernel::format::{PageId, RelId, RowId};
use redlinedb_kernel::txn::Isolation;
use redlinedb_kernel::wal::WalConfig;

const ROWS: u64 = 120;

fn config(pool_pages: usize) -> EngineConfig {
    EngineConfig {
        rel_id: RelId(1),
        commit_durability: CommitDurability::Normal,
        busy_timeout: Duration::from_millis(250),
        heap_lanes: 1,
        page_size: 4096,
        buffer_pool_pages: pool_pages,
        wal: WalConfig {
            group_commit_delay_us: 0,
            ..WalConfig::default()
        },
        ..EngineConfig::default()
    }
}

fn payload(i: u64) -> Vec<u8> {
    let mut bytes = vec![(i % 251) as u8; 1024];
    bytes[..8].copy_from_slice(&i.to_le_bytes());
    bytes
}

/// Copies of each committed row a scan of every heap page finds, by tag.
fn copies(engine: &Engine) -> BTreeMap<u64, usize> {
    let tx = engine.begin(Isolation::Snapshot).unwrap();
    let pages = engine
        .relation_entries(RelId(1))
        .unwrap()
        .iter()
        .map(|(_, ptr)| ptr.page_id.0)
        .fold(engine.heap_page_count().unwrap(), u64::max);
    let mut copies = BTreeMap::new();
    for row in engine
        .parallel_scan_page_range(
            tx.snapshot(),
            Some(tx.id()),
            PageId(1)..PageId(pages + 1),
            Some(RelId(1)),
            1,
            None,
        )
        .unwrap()
    {
        let tag = u64::from_le_bytes(row.payload[..8].try_into().unwrap());
        *copies.entry(tag).or_insert(0) += 1;
    }
    copies
}

#[test]
fn a_crash_during_heap_replay_leaves_one_copy_of_each_row() {
    let scenario = fail::FailScenario::setup();
    let temp = tempfile::tempdir().unwrap();
    let engine = Engine::create(temp.path(), config(1024)).unwrap();
    let mut rows: Vec<(RowId, u64)> = Vec::new();
    for i in 0..ROWS {
        let mut tx = engine.begin(Isolation::Snapshot).unwrap();
        rows.push((engine.insert(&mut tx, payload(i)).unwrap(), i));
        engine.commit(tx).unwrap();
    }
    drop(engine);
    let data = temp.path().join(config(1).data_file_name);
    let before = std::fs::metadata(&data).unwrap().len();

    failpoints::cfg("engine::recovery::after_heap_replay", "panic").unwrap();
    let crashed = std::panic::catch_unwind(|| Engine::open(temp.path(), config(16)).map(drop));
    failpoints::cfg("engine::recovery::after_heap_replay", "off").unwrap();
    drop(scenario);
    assert!(crashed.is_err(), "the recovery failpoint never fired");
    assert!(
        std::fs::metadata(&data).unwrap().len() > before,
        "replay wrote no page before the crash"
    );

    for reopen in 0..2 {
        let reopened = Engine::open(temp.path(), config(16)).unwrap();
        let expected: BTreeMap<u64, usize> = (0..ROWS).map(|i| (i, 1)).collect();
        assert_eq!(copies(&reopened), expected, "reopen {reopen}");
        let mut tx = reopened.begin(Isolation::Snapshot).unwrap();
        for (row, i) in &rows {
            assert_eq!(reopened.get(&mut tx, *row).unwrap(), Some(payload(*i)));
        }
    }
}
