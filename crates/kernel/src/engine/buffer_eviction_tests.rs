//! Buffer pool eviction keeps the WAL ahead of the page file.
//!
//! A dirty page may leave the pool only once the WAL records behind its page
//! LSN are durable. These tests fill a small pool with dirty pages and check
//! every page write against the WAL's durable LSN. The page-write hook is per
//! thread, and eviction runs on the thread that asked for a frame.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;

use super::{CommitDurability, Engine, EngineConfig};
use crate::catalog::{
    ColumnSpec, CreateIndexSpec, CreateTableSpec, DbName, IndexColumnSpec, IndexId, IndexOrigin,
    QualifiedName, SortDir,
};
use crate::format::{Lsn, PageGeneration, PageId, RelId, RowId, TuplePtr};
use crate::index::IndexRowRef;
use crate::storage::buffer_test_hooks::{page_file_syncs, set_before_page_write_hook};
use crate::txn::Isolation;
use crate::wal::WalConfig;
use crate::{Error, Result};

const SMALL_POOL: usize = 16;
const PAGE_SIZE: usize = 4096;
/// 1 KiB rows fill about 40 heap pages of 4 KiB, well past `SMALL_POOL`.
const ROWS: usize = 120;
/// 200-byte keys fill about 40 index leaves.
const KEYS: usize = 600;

const EVERY_DURABILITY: [CommitDurability; 3] = [
    CommitDurability::Strict,
    CommitDurability::Normal,
    CommitDurability::UnsafeDev,
];

fn config(durability: CommitDurability, pool_pages: usize) -> EngineConfig {
    EngineConfig {
        rel_id: RelId(1),
        commit_durability: durability,
        busy_timeout: Duration::from_millis(250),
        heap_lanes: 1,
        page_size: PAGE_SIZE,
        buffer_pool_pages: pool_pages,
        wal: WalConfig {
            group_commit_delay_us: 0,
            ..WalConfig::default()
        },
        ..EngineConfig::default()
    }
}

fn payload(i: usize) -> Vec<u8> {
    let mut bytes = vec![(i % 251) as u8; 1024];
    bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
    bytes
}

/// (page, page LSN, durable WAL LSN) for each page written ahead of its WAL.
type EarlyWrites = Rc<RefCell<Vec<(PageId, Lsn, Lsn)>>>;

/// Record every page this thread writes while the WAL is not yet durable
/// through that page's LSN.
fn watch_page_writes(engine: &Engine) -> EarlyWrites {
    let early = EarlyWrites::default();
    let sink = Rc::clone(&early);
    let wal = Arc::clone(&engine.wal);
    set_before_page_write_hook(Some(Box::new(move |page_id, page_lsn| {
        let durable = wal.durable_lsn().unwrap_or(Lsn::ZERO);
        if page_lsn > durable {
            sink.borrow_mut().push((page_id, page_lsn, durable));
        }
    })));
    early
}

/// Insert each row in its own committed transaction.
fn insert_rows(engine: &Engine, range: Range<usize>) -> Result<Vec<(RowId, usize)>> {
    let mut rows = Vec::with_capacity(range.len());
    for i in range {
        let mut tx = engine.begin(Isolation::Snapshot)?;
        let row = engine.insert(&mut tx, payload(i))?;
        engine.commit(tx)?;
        rows.push((row, i));
    }
    Ok(rows)
}

fn assert_rows(engine: &Engine, rows: &[(RowId, usize)]) {
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    for (row, i) in rows {
        assert_eq!(
            engine.get(&mut tx, *row).unwrap(),
            Some(payload(*i)),
            "row {row:?}"
        );
    }
}

#[test]
fn buffer_eviction_in_normal_mode_syncs_the_wal_before_the_page_write() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config(CommitDurability::Normal, SMALL_POOL)).unwrap();
    let early = watch_page_writes(&engine);
    let inserted = insert_rows(&engine, 0..ROWS);
    set_before_page_write_hook(None);
    let rows = inserted.unwrap();

    let stats = engine.buffer.stats();
    assert!(
        stats.evictions > 0 && stats.writes > 0,
        "the pool never wrote a dirty page out: {stats:?}"
    );
    assert_eq!(
        early.borrow().as_slice(),
        &[],
        "pages written before their WAL was durable (page, page lsn, durable lsn)"
    );
    assert_rows(&engine, &rows);
}

#[test]
fn buffer_eviction_admits_a_page_into_a_full_dirty_pool_in_every_durability_mode() {
    for durability in EVERY_DURABILITY {
        let temp = TempDir::new().unwrap();
        let engine = Engine::create(temp.path(), config(durability, SMALL_POOL)).unwrap();
        let early = watch_page_writes(&engine);
        let inserted = insert_rows(&engine, 0..ROWS);
        set_before_page_write_hook(None);
        let rows = inserted.unwrap_or_else(|err| {
            panic!("{durability:?}: an insert into a pool of dirty pages failed: {err:?}")
        });

        assert!(
            engine.buffer.stats().evictions > 0,
            "{durability:?}: the pool never evicted"
        );
        assert_eq!(
            early.borrow().as_slice(),
            &[],
            "{durability:?}: pages written before their WAL was durable"
        );
        assert_rows(&engine, &rows);
    }
}

#[test]
fn buffer_eviction_frees_index_pages_that_recovery_dirtied() {
    let temp = TempDir::new().unwrap();
    // The first run has room for every page, so nothing is evicted and each
    // leaf reaches the reopened pool only through WAL replay. Normal commits
    // keep the test fast; the shutdown flush still makes the WAL durable.
    let engine = Engine::create(temp.path(), config(CommitDurability::Normal, 1024)).unwrap();
    let index_id = create_indexed_table(&engine);
    let index = engine.index_handle(index_id).unwrap();
    for i in 0..KEYS {
        let mut tx = engine.begin(Isolation::Snapshot).unwrap();
        index
            .insert_tx(tx.id(), &index_key(i), index_row(i))
            .unwrap();
        engine.commit(tx).unwrap();
    }
    assert_eq!(engine.buffer.stats().evictions, 0);
    drop(index);
    drop(engine);

    // Replay dirties more index leaves, stamped with their WAL LSNs, than the
    // pool holds, and every key must still be found through the small pool.
    let reopened = Engine::open(temp.path(), config(CommitDurability::Normal, SMALL_POOL))
        .unwrap_or_else(|err| panic!("recovery into a {SMALL_POOL}-page pool failed: {err:?}"));
    assert_index_keys(&reopened, index_id);
    assert!(reopened.buffer.stats().evictions > 0);

    // Commits after recovery keep evicting, and a second reopen sees them.
    let rows = insert_rows(&reopened, 0..ROWS).unwrap();
    drop(reopened);
    let again = Engine::open(temp.path(), config(CommitDurability::Normal, SMALL_POOL)).unwrap();
    assert_index_keys(&again, index_id);
    assert_rows(&again, &rows);
}

#[test]
fn buffer_eviction_after_open_finds_the_replayed_wal_durable() {
    // UnsafeDev skips the shutdown fsync, so the reopened WAL may be written
    // but not durable. Replay stamps heap pages with LSN zero, which gives
    // eviction no LSN to ask the WAL about. Open makes the scanned WAL
    // durable before replay writes a page.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config(CommitDurability::UnsafeDev, 1024)).unwrap();
    let rows = insert_rows(&engine, 0..ROWS).unwrap();
    drop(engine);

    let (reopened, report) = Engine::open_with_recovery_report(
        temp.path(),
        config(CommitDurability::Strict, SMALL_POOL),
    )
    .unwrap();
    assert!(report.valid_end_lsn > Lsn::ZERO);
    assert!(
        reopened.buffer.stats().evictions > 0,
        "replay never evicted a page"
    );
    assert!(
        reopened.wal.durable_lsn().unwrap() >= report.valid_end_lsn,
        "open left the replayed WAL undurable"
    );
    assert_rows(&reopened, &rows);
}

#[test]
fn buffer_eviction_write_is_synced_by_the_next_checkpoint() {
    // Eviction writes a page without syncing the page file. A checkpoint
    // records an LSN past that page's WAL record and prunes the WAL below
    // it, so it has to sync the evicted write even when it flushes nothing.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config(CommitDurability::Normal, SMALL_POOL)).unwrap();
    let rows = insert_rows(&engine, 0..ROWS).unwrap();
    engine.checkpoint().unwrap();

    // One dirty page, then cold reads until eviction has written it out.
    let late = insert_rows(&engine, ROWS..ROWS + 1).unwrap();
    let writes = engine.buffer.stats().writes;
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    for (row, _) in rows.iter().chain(rows.iter()) {
        engine.get(&mut tx, *row).unwrap();
        if engine.buffer.stats().writes > writes {
            break;
        }
    }
    drop(tx);
    assert_eq!(
        engine.buffer.stats().writes,
        writes + 1,
        "eviction did not write the dirty page"
    );

    let syncs = page_file_syncs();
    let checkpoint = engine.checkpoint_with_stats().unwrap();
    assert_eq!(checkpoint.flushed_pages, 0);
    assert!(
        page_file_syncs() > syncs,
        "the checkpoint left the evicted write unsynced"
    );
    assert_rows(&engine, &late);
}

#[test]
fn buffer_eviction_of_an_open_transaction_keeps_its_ids_from_new_ones() {
    // Eviction can write pages holding tuples of a transaction that never
    // commits, with no checkpoint to record its id. Recovery must not hand
    // that transaction id, or its row ids, to new work: a reused transaction
    // id would make the orphaned tuples visible once the new one commits.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config(CommitDurability::Normal, SMALL_POOL)).unwrap();
    insert_rows(&engine, 0..3).unwrap();
    let mut open = engine.begin(Isolation::Snapshot).unwrap();
    let open_id = open.id();
    let mut open_rows = Vec::new();
    for i in 3..ROWS {
        open_rows.push(engine.insert(&mut open, payload(i)).unwrap());
    }
    assert!(
        engine.buffer.stats().writes > 0,
        "eviction never wrote the open transaction's pages"
    );
    drop(open);
    drop(engine);

    let reopened = Engine::open(temp.path(), config(CommitDurability::Normal, SMALL_POOL)).unwrap();
    let mut tx = reopened.begin(Isolation::Snapshot).unwrap();
    let new_id = tx.id();
    let new_row = reopened.insert(&mut tx, b"new".to_vec()).unwrap();
    reopened.commit(tx).unwrap();
    assert!(
        new_id > open_id,
        "{new_id:?} reuses the uncommitted {open_id:?}"
    );
    assert!(
        open_rows.iter().all(|row| new_row > *row),
        "{new_row:?} reuses a row id of the uncommitted transaction"
    );

    let tx = reopened.begin(Isolation::Snapshot).unwrap();
    let pages = reopened.heap_page_count().unwrap();
    let scanned = reopened
        .parallel_scan_page_range(
            tx.snapshot(),
            Some(tx.id()),
            PageId(1)..PageId(pages + 1),
            Some(RelId(1)),
            1,
            None,
        )
        .unwrap();
    let open_payloads: Vec<Vec<u8>> = (3..ROWS).map(payload).collect();
    assert!(
        scanned
            .iter()
            .all(|row| !open_payloads.contains(&row.payload)),
        "a scan shows rows of the transaction that never committed"
    );
}

#[test]
fn buffer_eviction_without_a_wal_keeps_a_page_ahead_of_durability() {
    // A pool that no engine attached a WAL to cannot prove a page's WAL is
    // durable, so the dirty page stays and allocation fails closed.
    let temp = TempDir::new().unwrap();
    let file = Arc::new(
        crate::storage::PageFile::create(temp.path().join("data.redline"), PAGE_SIZE).unwrap(),
    );
    let pool = crate::storage::BufferPool::new(file, 1).unwrap();
    let first = pool
        .allocate(crate::format::PageKind::Heap, RelId(1))
        .unwrap();
    first.mark_dirty(Lsn(10)).unwrap();
    drop(first);
    assert_eq!(
        pool.allocate(crate::format::PageKind::Heap, RelId(1))
            .unwrap_err(),
        Error::CorruptPage("no unpinned frame available for eviction")
    );
}

fn index_key(i: usize) -> Vec<u8> {
    let mut key = format!("{i:08}").into_bytes();
    key.resize(200, b'k');
    key
}

fn index_row(i: usize) -> IndexRowRef {
    IndexRowRef::with_row_id(
        RowId(i as u64 + 1),
        TuplePtr::new_with_generation(PageId(1), i as u16, PageGeneration::ONE),
    )
}

fn assert_index_keys(engine: &Engine, index_id: IndexId) {
    let index = engine.index_handle(index_id).expect("index handle");
    for i in 0..KEYS {
        assert_eq!(
            index.point_lookup(&index_key(i)).unwrap(),
            vec![index_row(i)],
            "key {i}"
        );
    }
}

fn create_indexed_table(engine: &Engine) -> IndexId {
    let column = |name: &str| ColumnSpec {
        name: DbName::new(name),
        declared_type: Some("TEXT".to_owned()),
        constraints: vec![],
        collation: None,
        default_value: None,
        autoincrement: false,
        generated: None,
    };
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    engine
        .create_table(
            &mut tx,
            CreateTableSpec {
                schema: None,
                name: DbName::new("t"),
                if_not_exists: false,
                columns: vec![column("v")],
                constraints: vec![],
                strict: false,
                without_rowid: false,
                normalized_sql: Some("CREATE TABLE t (v TEXT)".to_owned()),
            },
        )
        .unwrap();
    engine.commit(tx).unwrap();

    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    let index = engine
        .create_index(
            &mut tx,
            CreateIndexSpec {
                schema: None,
                name: DbName::new("ix_v"),
                if_not_exists: false,
                table: QualifiedName {
                    schema: DbName::new("main"),
                    name: DbName::new("t"),
                },
                unique: false,
                columns: vec![IndexColumnSpec {
                    name: DbName::new("v"),
                    sort_dir: SortDir::Asc,
                    collation: None,
                    expr_sql: None,
                    expr_referenced_cols: Vec::new(),
                }],
                origin: IndexOrigin::User,
                normalized_sql: Some("CREATE INDEX ix_v ON t(v)".to_owned()),
                predicate_sql: None,
            },
        )
        .unwrap();
    engine.commit(tx).unwrap();
    index.index_id
}
