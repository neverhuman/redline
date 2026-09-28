//! A page-range heap scan returns exactly the version of each row that the
//! snapshot sees.
//!
//! UPDATE and DELETE append a new tuple and leave the old one on its page
//! unchanged. The row directory names the newest tuple, and older versions
//! are reached through the undo chain. A scan that trusted every tuple on a
//! page returned superseded versions next to the current one and the last
//! version of each deleted row. The page count it scanned up to came from
//! the page file's length, so rows on pages that were still only in the
//! buffer pool were missed.
//!
//! Each test compares the scan, with one worker and with several, against
//! the serial row-by-row read and against a model kept by the test: before
//! any checkpoint (every page dirty and resident), after a checkpoint, after
//! new writes on top of a checkpoint, and after a reopen. Old snapshots,
//! a transaction's own writes, other open transactions and rolled-back
//! updates are covered too.

use std::collections::BTreeMap;
use std::time::Duration;

use redlinedb_kernel::engine::{CommitDurability, Engine, EngineConfig, Txn};
use redlinedb_kernel::format::{PageId, RelId, RowId};
use redlinedb_kernel::txn::Isolation;
use redlinedb_kernel::wal::WalConfig;
use tempfile::TempDir;

const REL: RelId = RelId(1);
const ROWS: u64 = 240;
const WORKERS: usize = 4;

/// The pool holds every page these tests write, so nothing reaches the page
/// file before a checkpoint.
fn config() -> EngineConfig {
    EngineConfig {
        rel_id: REL,
        commit_durability: CommitDurability::Strict,
        busy_timeout: Duration::from_millis(250),
        heap_lanes: 4,
        page_size: 4096,
        buffer_pool_pages: 1024,
        wal: WalConfig {
            group_commit_delay_us: 0,
            ..WalConfig::default()
        },
        ..EngineConfig::default()
    }
}

/// A 200-byte payload tagged with the row's key and version, so about 18
/// tuples fit on a page and the table spans many pages.
fn payload(key: u64, version: u64) -> Vec<u8> {
    let mut bytes = vec![(key % 251) as u8; 200];
    bytes[..8].copy_from_slice(&key.to_le_bytes());
    bytes[8..16].copy_from_slice(&version.to_le_bytes());
    bytes
}

fn tag(payload: &[u8]) -> (u64, u64) {
    (
        u64::from_le_bytes(payload[..8].try_into().unwrap()),
        u64::from_le_bytes(payload[8..16].try_into().unwrap()),
    )
}

/// Row id to payload for every row a reader should see.
type Model = BTreeMap<RowId, Vec<u8>>;
/// Row id to its payload's key and version: readable in a failed assertion.
type Tags = BTreeMap<RowId, (u64, u64)>;

fn tags(model: &Model) -> Tags {
    model
        .iter()
        .map(|(row, bytes)| (*row, tag(bytes)))
        .collect()
}

/// Row ids by key, so the workload can address rows by key.
type Keys = BTreeMap<u64, RowId>;

fn insert(engine: &Engine, keys: std::ops::Range<u64>, rows: &mut Keys, model: &mut Model) {
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    for key in keys {
        let bytes = payload(key, 0);
        let row = engine.insert(&mut tx, bytes.clone()).unwrap();
        rows.insert(key, row);
        model.insert(row, bytes);
    }
    engine.commit(tx).unwrap();
}

fn update(
    engine: &Engine,
    rows: &Keys,
    model: &mut Model,
    pick: impl Fn(u64) -> bool,
    version: u64,
) {
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    for (&key, &row) in rows {
        if pick(key) && model.contains_key(&row) {
            let bytes = payload(key, version);
            engine.update(&mut tx, row, bytes.clone()).unwrap();
            model.insert(row, bytes);
        }
    }
    engine.commit(tx).unwrap();
}

fn delete(engine: &Engine, rows: &Keys, model: &mut Model, pick: impl Fn(u64) -> bool) {
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    for (&key, &row) in rows {
        if pick(key) && model.remove(&row).is_some() {
            engine.delete(&mut tx, row).unwrap();
        }
    }
    engine.commit(tx).unwrap();
}

/// Insert `ROWS` rows, update every third row, update every sixth row a
/// second time, and delete every tenth row, each step in its own commit.
fn seed(engine: &Engine) -> (Keys, Model) {
    let mut rows = Keys::new();
    let mut model = Model::new();
    insert(engine, 0..ROWS, &mut rows, &mut model);
    update(engine, &rows, &mut model, |k| k % 3 == 0, 1);
    update(engine, &rows, &mut model, |k| k % 6 == 0, 2);
    delete(engine, &rows, &mut model, |k| k % 10 == 7);
    (rows, model)
}

/// Every heap page the engine has allocated, flushed or not.
fn all_pages(engine: &Engine) -> std::ops::Range<PageId> {
    PageId(1)..PageId(engine.heap_page_count().unwrap() + 1)
}

/// The page-range scan over every heap page as `tx` sees it, run over the
/// whole range and over two halves. A row returned twice fails here, naming
/// both versions.
fn page_scan(engine: &Engine, tx: &Txn, workers: usize) -> Model {
    let whole = scan_ranges(engine, tx, workers, &[all_pages(engine)]);
    let all = all_pages(engine);
    let mid = PageId(all.start.0 + (all.end.0 - all.start.0) / 2);
    let halves = scan_ranges(engine, tx, workers, &[all.start..mid, mid..all.end]);
    assert!(
        whole == halves,
        "{workers}-worker page scan: two half ranges disagree with the whole range"
    );
    whole
}

fn scan_ranges(
    engine: &Engine,
    tx: &Txn,
    workers: usize,
    ranges: &[std::ops::Range<PageId>],
) -> Model {
    let mut rows = Vec::new();
    for range in ranges {
        rows.extend(
            engine
                .parallel_scan_page_range(
                    tx.snapshot(),
                    Some(tx.id()),
                    range.clone(),
                    Some(REL),
                    workers,
                    None,
                )
                .unwrap(),
        );
    }
    let mut out = Model::new();
    for row in rows {
        assert_eq!(row.rel_id, REL);
        let new = tag(&row.payload);
        if let Some(old) = out.insert(row.row_id, row.payload) {
            panic!(
                "{workers}-worker page scan returned {:?} twice: (key, version) {:?} and {new:?}",
                row.row_id,
                tag(&old)
            );
        }
    }
    out
}

/// The serial read: each row in the relation directory, read as `tx` sees it.
fn serial_read(engine: &Engine, tx: &mut Txn) -> Model {
    let mut out = Model::new();
    for row in engine.relation_rowids(REL).unwrap() {
        if let Some(bytes) = engine.get_for_relation(tx, REL, row).unwrap() {
            out.insert(row, bytes);
        }
    }
    out
}

fn key_sum(model: &Model) -> u64 {
    model.values().map(|bytes| tag(bytes).0).sum()
}

/// Fail with the first few rows that differ, as (key, version) tags.
fn assert_same_rows(got: &Model, want: &Model, what: &str) {
    let (got_tags, want_tags) = (tags(got), tags(want));
    if got_tags == want_tags {
        return;
    }
    let missing: Vec<_> = want_tags
        .iter()
        .filter(|(row, _)| !got_tags.contains_key(row))
        .take(5)
        .collect();
    let extra: Vec<_> = got_tags
        .iter()
        .filter(|(row, _)| !want_tags.contains_key(row))
        .take(5)
        .collect();
    let wrong: Vec<_> = got_tags
        .iter()
        .filter_map(|(row, tag)| match want_tags.get(row) {
            Some(want) if want != tag => Some((row, tag, want)),
            _ => None,
        })
        .take(5)
        .collect();
    panic!(
        "{what}: {} rows, want {}; missing {missing:?}; extra {extra:?}; \
         wrong version (got, want) {wrong:?}",
        got.len(),
        want.len()
    );
}

/// `tx` must read `model` serially and through the page scan with one and
/// with several workers: the same rows, count, key sum and payload bytes.
fn assert_reads(engine: &Engine, tx: &mut Txn, model: &Model, label: &str) {
    let serial = serial_read(engine, tx);
    assert_same_rows(&serial, model, &format!("{label}: serial read"));
    for workers in [1, WORKERS] {
        let scanned = page_scan(engine, tx, workers);
        let what = format!("{label}: {workers}-worker page scan");
        assert_same_rows(&scanned, model, &what);
        assert_eq!(scanned.len(), model.len(), "{what}: count");
        assert_eq!(key_sum(&scanned), key_sum(model), "{what}: key sum");
        assert!(scanned == *model, "{what}: payload bytes");
    }
}

fn assert_fresh_reads(engine: &Engine, model: &Model, label: &str) {
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    assert_reads(engine, &mut tx, model, label);
    engine.rollback(tx).unwrap();
}

#[test]
fn page_scan_returns_one_visible_version_per_row_across_checkpoint_and_reopen() {
    let dir = TempDir::new().unwrap();
    let engine = Engine::create(dir.path(), config()).unwrap();
    let (mut rows, mut model) = seed(&engine);
    assert_eq!(
        engine.buffer_pool_stats().writes,
        0,
        "the table must still be only in the buffer pool"
    );
    assert_fresh_reads(&engine, &model, "before any checkpoint");

    engine.checkpoint().unwrap();
    assert_fresh_reads(&engine, &model, "after a checkpoint");

    // Change flushed pages again and add rows on pages past the file's end.
    update(&engine, &rows, &mut model, |k| k % 4 == 1, 3);
    insert(&engine, ROWS..ROWS + 60, &mut rows, &mut model);
    delete(&engine, &rows, &mut model, |k| k % 9 == 0);
    assert_fresh_reads(&engine, &model, "dirty pages on top of a checkpoint");

    drop(engine);
    let engine = Engine::open(dir.path(), config()).unwrap();
    assert_fresh_reads(&engine, &model, "after a reopen");
    engine.checkpoint().unwrap();
    assert_fresh_reads(&engine, &model, "after a checkpoint of the reopened engine");
}

#[test]
fn page_scan_reads_an_old_snapshot_through_the_undo_chain() {
    let dir = TempDir::new().unwrap();
    let engine = Engine::create(dir.path(), config()).unwrap();
    let (mut rows, mut model) = seed(&engine);
    let before = model.clone();
    let mut old = engine.begin(Isolation::Snapshot).unwrap();
    assert_reads(
        &engine,
        &mut old,
        &before,
        "old snapshot before later commits",
    );

    // Rows already updated twice get a third version; others their first.
    update(&engine, &rows, &mut model, |k| k % 2 == 0, 5);
    update(&engine, &rows, &mut model, |k| k % 6 == 0, 6);
    delete(&engine, &rows, &mut model, |k| k % 10 == 3);
    insert(&engine, ROWS..ROWS + 20, &mut rows, &mut model);

    assert_reads(
        &engine,
        &mut old,
        &before,
        "old snapshot after later commits",
    );
    assert_fresh_reads(&engine, &model, "new snapshot after later commits");

    engine.checkpoint().unwrap();
    assert_reads(
        &engine,
        &mut old,
        &before,
        "old snapshot after a checkpoint",
    );
    assert_fresh_reads(&engine, &model, "new snapshot after a checkpoint");
    engine.rollback(old).unwrap();
}

#[test]
fn page_scan_sees_its_own_writes_but_not_other_open_or_rolled_back_ones() {
    let dir = TempDir::new().unwrap();
    let engine = Engine::create(dir.path(), config()).unwrap();
    let (rows, committed) = seed(&engine);

    // An open writer: its view includes its own changes.
    let mut writer = engine.begin(Isolation::Snapshot).unwrap();
    let mut own = committed.clone();
    for (&key, &row) in &rows {
        if !own.contains_key(&row) {
            continue;
        }
        if key % 4 == 0 {
            let bytes = payload(key, 7);
            engine.update(&mut writer, row, bytes.clone()).unwrap();
            own.insert(row, bytes);
        } else if key % 10 == 1 {
            engine.delete(&mut writer, row).unwrap();
            own.remove(&row);
        }
    }
    for key in ROWS..ROWS + 20 {
        let bytes = payload(key, 7);
        let row = engine.insert(&mut writer, bytes.clone()).unwrap();
        own.insert(row, bytes);
    }

    assert_reads(&engine, &mut writer, &own, "the writer's own view");
    assert_fresh_reads(&engine, &committed, "another reader during the write");
    engine.checkpoint().unwrap();
    assert_reads(
        &engine,
        &mut writer,
        &own,
        "the writer's view after a checkpoint",
    );
    assert_fresh_reads(&engine, &committed, "another reader after a checkpoint");

    // The rolled-back versions stay the directory's newest tuples; readers
    // must reach the committed version beneath each through the undo chain.
    engine.rollback(writer).unwrap();
    assert_fresh_reads(&engine, &committed, "after the rollback");
    drop(engine);
    let engine = Engine::open(dir.path(), config()).unwrap();
    assert_fresh_reads(&engine, &committed, "after the rollback and a reopen");
}
