//! Heap redo finds the row each WAL record changes without a page scan.
//!
//! Recovery loaded the row directory from the heap pages only after heap
//! redo, so redoing an update or a delete looked for its row by reading
//! every heap page from the first until it found it. Reopening after N
//! updates cost N scans of the table. The directory is now loaded from the
//! checkpointed pages first, and each redo is a directory lookup.

use std::sync::Mutex;
use std::time::Duration;

use redlinedb_kernel::engine::{CommitDurability, Engine, EngineConfig};
use redlinedb_kernel::format::{DEFAULT_PAGE_SIZE, RelId, RowId};
use redlinedb_kernel::observe;
use redlinedb_kernel::txn::Isolation;
use redlinedb_kernel::wal::WalConfig;
use tempfile::TempDir;

/// Rows in the table: enough heap pages that a scan per update shows.
const ROWS: u64 = 4_000;
/// Updates left in the WAL after the checkpoint.
const UPDATES: u64 = 50;

/// The counters are process-wide; under `cargo test` the tests of this
/// binary share a process.
static COUNTING: Mutex<()> = Mutex::new(());

fn config() -> EngineConfig {
    EngineConfig {
        rel_id: RelId(1),
        wal: WalConfig::default(),
        commit_durability: CommitDurability::Normal,
        lock_shards: 32,
        busy_timeout: Duration::from_millis(250),
        heap_lanes: 4,
        page_size: DEFAULT_PAGE_SIZE,
        buffer_pool_pages: 4_096,
        data_file_name: "data.redline".to_owned(),
    }
}

fn payload(row: u64, version: &str) -> Vec<u8> {
    format!("{version}:{row:08}:{}", "x".repeat(200)).into_bytes()
}

/// A checkpointed table of [`ROWS`] rows, then `updates` committed updates
/// of the last rows (the ones a scan from the first page reaches last).
fn database(updates: u64) -> (TempDir, Vec<RowId>) {
    let temp = TempDir::new().expect("temp dir");
    let engine = Engine::create(temp.path(), config()).expect("create");
    let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
    let rows: Vec<RowId> = (0..ROWS)
        .map(|row| engine.insert(&mut tx, payload(row, "v1")).expect("insert"))
        .collect();
    engine.commit(tx).expect("commit inserts");
    engine.checkpoint().expect("checkpoint");
    for (index, row) in rows.iter().rev().take(updates as usize).enumerate() {
        let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
        let n = ROWS - 1 - index as u64;
        engine
            .update(&mut tx, *row, payload(n, "v2"))
            .expect("update");
        engine.commit(tx).expect("commit update");
    }
    drop(engine);
    (temp, rows)
}

/// Heap page pins while `Engine::open` recovers `dir`.
fn pins_to_open(dir: &TempDir) -> (std::sync::Arc<Engine>, u64) {
    let before = observe::snapshot();
    let engine = Engine::open(dir.path(), config()).expect("open");
    (engine, observe::snapshot().since(before).heap_page_pins)
}

#[test]
fn reopening_after_updates_reads_each_updated_row_once() {
    let _guard = COUNTING.lock().unwrap_or_else(|p| p.into_inner());
    let (clean, _) = database(0);
    let (_, clean_pins) = pins_to_open(&clean);

    let (updated, rows) = database(UPDATES);
    let (engine, updated_pins) = pins_to_open(&updated);

    let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
    for (index, row) in rows.iter().enumerate() {
        let n = index as u64;
        let version = if n >= ROWS - UPDATES { "v2" } else { "v1" };
        assert_eq!(
            engine.get(&mut tx, *row).expect("get"),
            Some(payload(n, version)),
            "row {n} after reopen"
        );
    }

    let per_update = updated_pins.saturating_sub(clean_pins) as f64 / UPDATES as f64;
    assert!(
        per_update <= 8.0,
        "reopening after {UPDATES} updates pinned {updated_pins} heap pages, \
         {clean_pins} without them: {per_update:.1} pins per replayed update"
    );
}
