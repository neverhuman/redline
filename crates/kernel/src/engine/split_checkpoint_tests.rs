//! A checkpoint taken during a B-tree split stays behind the split.
//!
//! A split appends a page image for each page it changes (both leaves, a
//! parent or a new root, and the meta page) and installs those pages only
//! after the appends. Recovery skips records older than the checkpoint, and
//! the checkpoint writes only pages that are already installed, so the
//! checkpoint must not pass the split's first image until every page of the
//! split is installed. These tests take one checkpoint at each image append
//! of a split, through the page-install hook, then reopen without another.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use tempfile::TempDir;

use super::buffer_eviction_tests::create_indexed_table;
use super::{CommitDurability, Engine, EngineConfig};
use crate::format::PageKind::{BtreeInternal as Internal, BtreeLeaf as Leaf, BtreeMeta as Meta};
use crate::format::{Page, PageGeneration, PageId, PageKind, RowId, TuplePtr};
use crate::index::IndexRowRef;
use crate::txn::Isolation;
use crate::wal::{WalConfig, WalPayload, WalReader, WalRecordKind, set_before_page_install_hook};

const PAGE_SIZE: usize = 4096;
/// 200-byte keys fit eight to a leaf and 17 separators to an internal
/// page, so these ascending keys split leaves under a new root, under that
/// root, and finally split the root itself.
const KEYS: usize = 80;
const KEY_LEN: usize = 200;

/// Each split shape with the page images it appends, in order.
const SHAPES: [(&str, &[PageKind]); 3] = [
    ("leaf split with a new root", &[Leaf, Leaf, Internal, Meta]),
    ("leaf split under a parent", &[Leaf, Leaf, Internal]),
    (
        "leaf and parent split with a new root",
        &[Leaf, Leaf, Internal, Internal, Internal, Meta],
    ),
];

/// Normal commits keep the test fast. Each checkpoint flushes the WAL, and
/// so does shutdown before the reopen.
fn config() -> EngineConfig {
    EngineConfig {
        commit_durability: CommitDurability::Normal,
        page_size: PAGE_SIZE,
        buffer_pool_pages: 256,
        wal: WalConfig {
            group_commit_delay_us: 0,
            ..WalConfig::default()
        },
        ..EngineConfig::default()
    }
}

fn key(i: usize) -> Vec<u8> {
    let mut key = format!("{i:08}").into_bytes();
    key.resize(KEY_LEN, b'k');
    key
}

fn row(i: usize) -> IndexRowRef {
    IndexRowRef::with_row_id(
        RowId(i as u64 + 1),
        TuplePtr::new_with_generation(PageId(1), i as u16, PageGeneration::ONE),
    )
}

/// Run `on_install` at every page-install hook on this thread until the
/// hook is cleared. The hook disarms itself when it runs, so re-arm it.
fn hook_every_install(on_install: Rc<dyn Fn()>) {
    set_before_page_install_hook(Some(Box::new(move || {
        on_install();
        hook_every_install(Rc::clone(&on_install));
    })));
}

/// Page installs each insert of `0..KEYS` reaches: one for a plain insert,
/// one per page image for a split.
fn installs_per_insert() -> Vec<usize> {
    let dir = TempDir::new().unwrap();
    let engine = Engine::create(dir.path(), config()).unwrap();
    let index = engine
        .index_handle(create_indexed_table(&engine))
        .expect("index handle");
    let calls = Rc::new(Cell::new(0_usize));
    let counter = Rc::clone(&calls);
    hook_every_install(Rc::new(move || counter.set(counter.get() + 1)));
    let mut per_insert = Vec::with_capacity(KEYS);
    for i in 0..KEYS {
        calls.set(0);
        let tx = engine.begin(Isolation::Snapshot).unwrap();
        index.insert_tx(tx.id(), &key(i), row(i)).unwrap();
        engine.commit(tx).unwrap();
        per_insert.push(calls.get());
    }
    set_before_page_install_hook(None);
    per_insert
}

/// Insert keys `0..=last`, take a checkpoint at page install `at` of the
/// last insert, which splits, and reopen without another checkpoint.
/// Returns every way the checkpoint or the reopened index went wrong.
fn checkpoint_at_install(last: usize, at: usize, images: &[PageKind]) -> Vec<String> {
    let dir = TempDir::new().unwrap();
    let engine = Engine::create(dir.path(), config()).unwrap();
    let index_id = create_indexed_table(&engine);
    let index = engine.index_handle(index_id).expect("index handle");
    for i in 0..last {
        let tx = engine.begin(Isolation::Snapshot).unwrap();
        index.insert_tx(tx.id(), &key(i), row(i)).unwrap();
        engine.commit(tx).unwrap();
    }

    let calls = Rc::new(Cell::new(0_usize));
    let seen = Rc::new(Cell::new(None));
    let (hook_calls, hook_seen, hook_engine) =
        (Rc::clone(&calls), Rc::clone(&seen), Arc::clone(&engine));
    hook_every_install(Rc::new(move || {
        hook_calls.set(hook_calls.get() + 1);
        if hook_calls.get() == at {
            let checkpoint = hook_engine
                .checkpoint()
                .expect("checkpoint while a split page is uninstalled");
            hook_seen.set(Some(checkpoint.checkpoint_lsn));
        }
    }));
    let tx = engine.begin(Isolation::Snapshot).unwrap();
    let split_tx = tx.id();
    let inserted = index.insert_tx(split_tx, &key(last), row(last));
    set_before_page_install_hook(None);
    inserted.unwrap();
    engine.commit(tx).unwrap();
    let during = seen
        .get()
        .unwrap_or_else(|| panic!("insert {last} never reached page install {at}"));

    let records = WalReader::new(&engine.wal_dir, engine.config.wal.clone())
        .scan()
        .unwrap();
    let split_images: Vec<_> = records
        .iter()
        .filter(|record| record.tx_id == split_tx && record.kind == WalRecordKind::PageImage)
        .map(
            |record| match WalPayload::decode(&record.payload).unwrap() {
                WalPayload::PageImage { page_bytes, .. } => {
                    let kind = Page::from_bytes(page_bytes).unwrap().header().unwrap().kind;
                    (record.lsn, kind)
                }
                other => panic!("page image record holds {other:?}"),
            },
        )
        .collect();
    let kinds: Vec<_> = split_images.iter().map(|(_, kind)| *kind).collect();
    assert_eq!(kinds, images, "insert {last} split a different shape");
    let first_image = split_images[0].0;

    let mut problems = Vec::new();
    if during > first_image {
        problems.push(format!(
            "checkpoint {during:?} passed the first split image {first_image:?}"
        ));
    }
    drop(index);
    drop(engine);
    let reopened = match Engine::open(dir.path(), config()) {
        Ok(reopened) => reopened,
        Err(err) => {
            problems.push(format!("reopen failed: {err:?}"));
            return problems;
        }
    };
    let index = reopened.index_handle(index_id).expect("reopened index");
    for i in 0..=last {
        match index.point_lookup(&key(i)) {
            Ok(rows) if rows == vec![row(i)] => {}
            other => problems.push(format!("key {i} after reopen: {other:?}")),
        }
    }
    match index.validate() {
        Ok(report) if report.errors.is_empty() => {}
        other => problems.push(format!("validate after reopen: {other:?}")),
    }
    problems
}

#[test]
fn checkpoint_does_not_pass_an_uninstalled_split_page() {
    let per_insert = installs_per_insert();
    let mut problems = Vec::new();
    for (shape, images) in SHAPES {
        let last = per_insert
            .iter()
            .position(|&installs| installs == images.len())
            .unwrap_or_else(|| panic!("no insert made a {shape}: {per_insert:?}"));
        for at in 1..=images.len() {
            for problem in checkpoint_at_install(last, at, images) {
                problems.push(format!(
                    "{shape} (insert {last}), checkpoint at image {at} of {}: {problem}",
                    images.len()
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
