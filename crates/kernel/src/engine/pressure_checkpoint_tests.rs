//! Checkpoints that eviction asks for, under many concurrent writers.
//!
//! When every unpinned frame holds a dirty page it may not write on its own,
//! eviction asks the engine for a checkpoint. Before checkpoints took a
//! complete cut (workplan R5), one that ran while another thread wrote to a
//! page it had already chosen to skip lost that page's older rows, so these
//! checkpoints were allowed only with a single writer. Here many writers
//! share a small pool: no insert may fail, and every committed row and index
//! key must come back exactly once after a reopen.

use std::collections::BTreeMap;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use super::buffer_eviction_tests::{create_indexed_table, scratch_dir};
use super::{CommitDurability, Engine, EngineConfig};
use crate::catalog::IndexId;
use crate::format::{PageGeneration, PageId, RelId, RowId, TuplePtr};
use crate::index::IndexRowRef;
use crate::txn::Isolation;
use crate::wal::WalConfig;

const ROWS_PER_WRITER: usize = 150;
/// (writers, pool pages). Each run writes far more pages than the pool: 150
/// rows of 600 bytes per writer on 4 KiB pages, and an index with a 100-byte
/// key per row. 24 frames is the pool a Phase 1 stress run split an index in
/// with "expected internal page".
const SHAPES: [(usize, usize); 2] = [(8, 48), (4, 24)];
const DEADLOCK_GUARD: Duration = Duration::from_secs(300);

fn config(pool_pages: usize) -> EngineConfig {
    EngineConfig {
        rel_id: RelId(1),
        commit_durability: CommitDurability::Normal,
        busy_timeout: Duration::from_secs(5),
        heap_lanes: 4,
        page_size: 4096,
        buffer_pool_pages: pool_pages,
        wal: WalConfig {
            group_commit_delay_us: 0,
            ..WalConfig::default()
        },
        ..EngineConfig::default()
    }
}

fn tag(run: usize, writer: usize, i: usize) -> u64 {
    (run * 100_000_000 + writer * 1_000_000 + i) as u64
}

fn payload(tag: u64, version: u8) -> Vec<u8> {
    let mut bytes = vec![version; 600];
    bytes[..8].copy_from_slice(&tag.to_le_bytes());
    bytes
}

fn key(tag: u64) -> Vec<u8> {
    let mut key = format!("{tag:012}").into_bytes();
    key.resize(100, b'k');
    key
}

fn entry(row: RowId) -> IndexRowRef {
    IndexRowRef::with_row_id(
        row,
        TuplePtr::new_with_generation(PageId(1), 0, PageGeneration::ONE),
    )
}

/// What a writer committed: each row's payload, or `None` once deleted.
type Committed = BTreeMap<RowId, (u64, Option<Vec<u8>>)>;

/// Insert rows with an index key each, update every fifth row and delete
/// every seventh, one transaction per change.
fn write(engine: &Engine, index_id: IndexId, run: usize, writer: usize) -> Committed {
    let index = engine.index_handle(index_id).expect("index handle");
    let mut committed = Committed::new();
    for i in 0..ROWS_PER_WRITER {
        let tag = tag(run, writer, i);
        let mut tx = engine.begin(Isolation::Snapshot).unwrap();
        let row = engine
            .insert(&mut tx, payload(tag, 1))
            .unwrap_or_else(|err| panic!("writer {writer} insert {i}: {err:?}"));
        index
            .insert_tx(tx.id(), &key(tag), entry(row))
            .unwrap_or_else(|err| panic!("writer {writer} index insert {i}: {err:?}"));
        engine.commit(tx).unwrap();
        committed.insert(row, (tag, Some(payload(tag, 1))));
        if i % 5 == 4 {
            // The commit above returned only once new snapshots see it
            // (workplan R8), so this snapshot sees the row it updates.
            let mut tx = engine.begin(Isolation::Snapshot).unwrap();
            engine
                .update(&mut tx, row, payload(tag, 2))
                .and_then(|_| engine.commit(tx).map(drop))
                .unwrap_or_else(|err| panic!("writer {writer} update {i}: {err:?}"));
            committed.insert(row, (tag, Some(payload(tag, 2))));
        }
        if i % 7 == 6 {
            let mut tx = engine.begin(Isolation::Snapshot).unwrap();
            engine
                .delete(&mut tx, row)
                .and_then(|_| engine.commit(tx).map(drop))
                .unwrap_or_else(|err| panic!("writer {writer} delete {i}: {err:?}"));
            committed.insert(row, (tag, None));
        }
    }
    committed
}

fn assert_committed(engine: &Engine, index_id: IndexId, committed: &Committed, when: &str) {
    let index = engine.index_handle(index_id).expect("index handle");
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    for (row, (tag, expected)) in committed {
        assert_eq!(
            engine.get(&mut tx, *row).unwrap().as_ref(),
            expected.as_ref(),
            "{when}: row {row:?} (tag {tag})"
        );
        assert_eq!(
            index.point_lookup(&key(*tag)).unwrap(),
            vec![entry(*row)],
            "{when}: index key of tag {tag}"
        );
    }
    let live = committed
        .values()
        .filter(|(_, value)| value.is_some())
        .count();
    let mut directory = engine.relation_entries(RelId(1)).unwrap();
    directory.retain(|(row, _)| {
        engine
            .get(&mut tx, *row)
            .map(|value| value.is_some())
            .unwrap_or(false)
    });
    assert_eq!(directory.len(), live, "{when}: live rows in the directory");
    // Reads by row id cannot see a second physical copy of a row, the
    // failure a checkpoint that wrote a heap change recovery replays again
    // would leave. A scan of every heap page can, for rows that were only
    // inserted: that scan also returns the superseded version of an
    // updated row, so updated and deleted rows are left out here.
    let copies = super::checkpoint_cut_tests::copies_on_pages(engine);
    for (row, (tag, expected)) in committed {
        if expected.as_deref() == Some(payload(*tag, 1).as_slice()) {
            assert_eq!(
                copies.get(row).copied().unwrap_or(0),
                1,
                "{when}: copies of inserted row {row:?} (tag {tag}) in a scan of every heap page"
            );
        }
    }
    let unexpected: Vec<&RowId> = copies
        .keys()
        .filter(|row| !committed.contains_key(row))
        .collect();
    assert!(
        unexpected.is_empty(),
        "{when}: a heap scan found rows no writer committed: {unexpected:?}"
    );
    assert_eq!(
        index.validate().unwrap().errors,
        Vec::<&str>::new(),
        "{when}"
    );
}

fn run_writers(engine: &Arc<Engine>, index_id: IndexId, writers: usize, run: usize) -> Committed {
    let writers: Vec<_> = (0..writers)
        .map(|writer| {
            let engine = Arc::clone(engine);
            thread::spawn(move || write(&engine, index_id, run, writer))
        })
        .collect();
    let mut committed = Committed::new();
    for writer in writers {
        committed.extend(writer.join().expect("writer panicked"));
    }
    committed
}

#[test]
fn pressure_checkpoints_keep_every_row_of_many_writers_in_a_small_pool() {
    let (done_tx, done) = mpsc::channel();
    let scenario = thread::spawn(move || {
        for (writers, pool_pages) in SHAPES {
            for round in 0..2 {
                let shape = format!("{writers} writers, {pool_pages} pages, round {round}");
                let temp = scratch_dir();
                let engine = Engine::create(temp.path(), config(pool_pages)).unwrap();
                engine.enable_pool_pressure_checkpoints().unwrap();
                let index_id = create_indexed_table(&engine);
                let mut committed = run_writers(&engine, index_id, writers, 0);
                let stats = engine.buffer_pool_stats();
                assert!(
                    stats.pressure_checkpoints > 0,
                    "{shape}: eviction never asked for a checkpoint: {stats:?}"
                );
                assert_committed(
                    &engine,
                    index_id,
                    &committed,
                    &format!("{shape}, before reopen"),
                );
                drop(engine);

                // Recovery replays into the same small pool, then more writers
                // run on the reopened engine before a second reopen.
                let reopened = Engine::open(temp.path(), config(pool_pages)).unwrap();
                reopened.enable_pool_pressure_checkpoints().unwrap();
                assert_committed(
                    &reopened,
                    index_id,
                    &committed,
                    &format!("{shape}, after reopen"),
                );
                committed.extend(run_writers(&reopened, index_id, writers, 1));
                assert_committed(
                    &reopened,
                    index_id,
                    &committed,
                    &format!("{shape}, second run"),
                );
                drop(reopened);
                let again = Engine::open(temp.path(), config(pool_pages)).unwrap();
                assert_committed(
                    &again,
                    index_id,
                    &committed,
                    &format!("{shape}, after second reopen"),
                );
            }
        }
        done_tx.send(()).unwrap();
    });
    done.recv_timeout(DEADLOCK_GUARD)
        .expect("writers and pressure checkpoints deadlocked");
    scenario.join().unwrap();
}
