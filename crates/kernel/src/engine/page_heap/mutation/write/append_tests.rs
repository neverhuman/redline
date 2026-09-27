use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::super::test_hooks::{
    PageHook, set_before_heap_install_hook, set_before_heap_wal_hook,
    set_before_tuple_overwrite_hook,
};
use super::PageBackedHeap;
use crate::Error;
use crate::engine::{Engine, EngineConfig};
use crate::format::{
    Lsn, Page, PageGeneration, PageId, PageKind, RelId, RowId, TuplePtr, TupleVersion, TxId,
    UndoPtr,
};
use crate::storage::{BufferPool, PageFile};
use crate::txn::Isolation;
use crate::wal::{WalConfig, WalCoordinator, WalPayload};

/// How long a hooked page write waits for a concurrent writer before it goes on.
/// A writer that finishes inside this window ran while the page should have been
/// latched.
const LATCH_WAIT: Duration = Duration::from_millis(250);

fn heap_with_wal(config: WalConfig) -> (tempfile::TempDir, PageBackedHeap) {
    let dir = tempfile::tempdir().unwrap();
    let page_file = Arc::new(PageFile::create(dir.path().join("data.redline"), 4096).unwrap());
    let buffer = Arc::new(BufferPool::new(Arc::clone(&page_file), 32).unwrap());
    let wal = Arc::new(WalCoordinator::create(dir.path().join("wal"), config).unwrap());
    let heap = PageBackedHeap::new_with_wal(RelId(1), 1, buffer, Some(wal)).unwrap();
    (dir, heap)
}

fn heap() -> (tempfile::TempDir, PageBackedHeap) {
    heap_with_wal(WalConfig::default())
}

fn payload(row: RowId) -> WalPayload {
    WalPayload::HeapInsert {
        tx_id: TxId(1),
        rel_id: RelId(1),
        row_id: row,
        payload: b"row".to_vec(),
    }
}

fn append(heap: &PageBackedHeap, row: RowId, tuple: TupleVersion) -> TuplePtr {
    heap.append_tuple(TxId(1), row, tuple, Lsn(1), Some(payload(row)))
        .unwrap()
}

fn resident(heap: &PageBackedHeap, page_id: PageId) -> (PageGeneration, u16) {
    heap.buffer
        .pin(page_id)
        .unwrap()
        .with_page(|page| Ok((page.header()?.generation, page.slot_count()?)))
        .unwrap()
}

/// Run `work` on another thread once `body` stops at the hook that `set_hook`
/// arms. Returns whether `work` finished while `body` was stopped there.
fn run_inside(
    set_hook: fn(Option<PageHook>),
    work: impl FnOnce() + Send + 'static,
    body: impl FnOnce(),
) -> bool {
    let (start_tx, start_rx) = mpsc::channel::<()>();
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let worker = thread::spawn(move || {
        if start_rx.recv().is_ok() {
            work();
            let _ = done_tx.send(());
        }
    });
    let inside = Arc::new(Mutex::new(None));
    let inside_hook = Arc::clone(&inside);
    set_hook(Some(Box::new(move |_resident: &Page| {
        start_tx.send(()).expect("start the concurrent writer");
        let finished = done_rx.recv_timeout(LATCH_WAIT).is_ok();
        *inside_hook.lock().expect("hook window") = Some(finished);
    })));
    body();
    set_hook(None);
    worker.join().expect("concurrent writer panicked");
    inside
        .lock()
        .expect("hook window")
        .expect("the page write never reached its hook")
}

/// Points inside a heap append, between copying the resident page and
/// installing the staged one, where a concurrent writer can start. The frame
/// latch has to cover all of them.
const APPEND_STAGES: [(&str, fn(Option<PageHook>)); 2] = [
    ("before its WAL append", set_before_heap_wal_hook),
    ("before its page install", set_before_heap_install_hook),
];

#[test]
fn fresh_append_publishes_one_cell() {
    let (_dir, heap) = heap();
    let row = RowId(1);
    let tuple = TupleVersion::new(row, RelId(1), TxId(1), b"row".to_vec());
    let ptr = append(&heap, row, tuple);
    assert_eq!(ptr.page_id, PageId(1));
    assert_eq!(resident(&heap, PageId(1)), (PageGeneration::ONE.next(), 1));
}

#[test]
fn fresh_page_is_reinitialised_only_on_the_private_copy() {
    let (_dir, heap) = heap();
    let seen = Arc::new(Mutex::new(None));
    let seen_hook = Arc::clone(&seen);
    set_before_heap_install_hook(Some(Box::new(move |resident: &Page| {
        let header = resident.header().unwrap();
        *seen_hook.lock().expect("resident page") =
            Some((header.generation, resident.slot_count().unwrap()));
    })));
    let row = RowId(1);
    let tuple = TupleVersion::new(row, RelId(1), TxId(1), b"row".to_vec());
    let ptr = append(&heap, row, tuple);
    set_before_heap_install_hook(None);

    // The WAL record is already appended here, and the resident page is still
    // the page the allocator built.
    let before_install = seen.lock().expect("resident page").take();
    assert_eq!(before_install, Some((PageGeneration::ONE, 0)));
    assert_eq!(ptr.generation, PageGeneration::ONE.next());
    assert_eq!(
        resident(&heap, ptr.page_id),
        (PageGeneration::ONE.next(), 1)
    );
}

#[test]
fn in_place_write_during_an_append_is_kept() {
    let (_dir, heap) = heap();
    let heap = Arc::new(heap);
    let first = RowId(1);
    let mut tuple = TupleVersion::new(first, RelId(1), TxId(1), b"first".to_vec());
    tuple.undo_head = UndoPtr((9 << 16) | 3);
    let first_ptr = append(&heap, first, tuple);
    // Vacuum drops the undo link of the head tuple in place.
    let mut pruned = heap.read_tuple(first_ptr).unwrap();
    pruned.undo_head = UndoPtr::ZERO;

    for (row, (stage, set_hook)) in (2..).map(RowId).zip(APPEND_STAGES) {
        let mut restored = heap.read_tuple(first_ptr).unwrap();
        restored.undo_head = UndoPtr((9 << 16) | 3);
        heap.overwrite_tuple(first_ptr, &restored).unwrap();

        let vacuum_heap = Arc::clone(&heap);
        let pruned = pruned.clone();
        let mut ptr = None;
        let finished_inside = run_inside(
            set_hook,
            move || vacuum_heap.overwrite_tuple(first_ptr, &pruned).unwrap(),
            || {
                let tuple = TupleVersion::new(row, RelId(1), TxId(1), b"second".to_vec());
                ptr = Some(append(&heap, row, tuple));
            },
        );

        assert_eq!(
            heap.read_tuple(first_ptr).unwrap().undo_head,
            UndoPtr::ZERO,
            "{stage}: the append installed a page copy taken before the in-place write"
        );
        assert!(
            !finished_inside,
            "{stage}: an in-place write ran between the page copy and its install"
        );
        let ptr = ptr.unwrap();
        assert_eq!(ptr.page_id, first_ptr.page_id);
        assert_eq!(heap.read_tuple(ptr).unwrap().payload, b"second");
    }
}

#[test]
fn checkpoint_during_a_heap_append_keeps_the_row_after_reopen() {
    let config = EngineConfig {
        buffer_pool_pages: 32,
        wal: WalConfig {
            group_commit_delay_us: 0,
            ..WalConfig::default()
        },
        ..EngineConfig::default()
    };
    for (stage, set_hook) in APPEND_STAGES {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::create(dir.path(), config.clone()).unwrap();
        let mut tx = engine.begin(Isolation::Snapshot).unwrap();
        let checkpoint_engine = Arc::clone(&engine);
        let mut row = None;
        let finished_inside = run_inside(
            set_hook,
            move || {
                checkpoint_engine.checkpoint().unwrap();
            },
            || row = Some(engine.insert(&mut tx, b"alpha".to_vec()).unwrap()),
        );
        engine.commit(tx).unwrap();
        // Drop without another checkpoint, as a crash would.
        drop(engine);

        let reopened = Engine::open(dir.path(), config.clone()).unwrap();
        let mut tx = reopened.begin(Isolation::Snapshot).unwrap();
        assert_eq!(
            reopened.get(&mut tx, row.unwrap()).unwrap(),
            Some(b"alpha".to_vec()),
            "{stage}: the checkpoint moved past the heap record before its page was installed"
        );
        assert!(
            !finished_inside,
            "{stage}: a checkpoint finished between a heap page copy and its install"
        );
    }
}

#[test]
fn failed_append_leaves_a_reused_page_for_the_next_reinitialisation() {
    let (_dir, heap) = heap_with_wal(WalConfig {
        wal_buffer_bytes: 1024,
        ..WalConfig::default()
    });
    let first = RowId(1);
    let old = append(
        &heap,
        first,
        TupleVersion::new(first, RelId(1), TxId(1), b"old".to_vec()),
    );
    // Vacuum found no head on the page and queued it for reuse.
    heap.append_lanes[0].lock().unwrap().heap_page = None;
    heap.push_reusable_page(PageKind::Heap, old.page_id)
        .unwrap();

    // A WAL record larger than the WAL buffer fails after the page is taken.
    let second = RowId(2);
    let oversized = WalPayload::HeapInsert {
        tx_id: TxId(2),
        rel_id: RelId(1),
        row_id: second,
        payload: vec![0; 2048],
    };
    let tuple = TupleVersion::new(second, RelId(1), TxId(2), b"lost".to_vec());
    let err = heap
        .append_tuple(TxId(2), second, tuple, Lsn(1), Some(oversized))
        .unwrap_err();
    assert!(matches!(err, Error::CorruptWal(_)), "{err:?}");
    assert_eq!(resident(&heap, old.page_id), (old.generation, 1));

    let third = RowId(3);
    let ptr = append(
        &heap,
        third,
        TupleVersion::new(third, RelId(1), TxId(3), b"new".to_vec()),
    );
    assert_eq!(ptr.page_id, old.page_id);
    assert_eq!(
        ptr.generation,
        old.generation.next(),
        "the lane appended to a reused page it never reinitialised"
    );
    assert_eq!(resident(&heap, old.page_id), (old.generation.next(), 1));
    assert!(heap.read_tuple(old).is_err());
}

#[test]
fn append_during_an_in_place_write_keeps_its_page_lsn() {
    let (_dir, heap) = heap();
    let heap = Arc::new(heap);
    let first = RowId(1);
    let mut tuple = TupleVersion::new(first, RelId(1), TxId(1), b"first".to_vec());
    tuple.undo_head = UndoPtr((9 << 16) | 3);
    let first_ptr = append(&heap, first, tuple);
    let mut pruned = heap.read_tuple(first_ptr).unwrap();
    pruned.undo_head = UndoPtr::ZERO;

    let append_heap = Arc::clone(&heap);
    let second = RowId(2);
    let (ptr_tx, ptr_rx) = mpsc::channel();
    let finished_inside = run_inside(
        set_before_tuple_overwrite_hook,
        move || {
            let tuple = TupleVersion::new(second, RelId(1), TxId(1), b"second".to_vec());
            ptr_tx.send(append(&append_heap, second, tuple)).unwrap();
        },
        || heap.overwrite_tuple(first_ptr, &pruned).unwrap(),
    );
    let second_ptr = ptr_rx.recv().unwrap();

    // The second append is the newest WAL record, so its end is the page LSN.
    let newest = heap.wal.as_ref().unwrap().flush_all().unwrap();
    let page_lsn = heap
        .buffer
        .pin(first_ptr.page_id)
        .unwrap()
        .with_page(|page| Ok(page.header()?.page_lsn))
        .unwrap();
    assert_eq!(
        page_lsn, newest,
        "the in-place write put back a page LSN it read before the append"
    );
    assert!(
        !finished_inside,
        "an append installed between the in-place write's page read and its write"
    );
    assert_eq!(second_ptr.page_id, first_ptr.page_id);
    assert_eq!(heap.read_tuple(first_ptr).unwrap().undo_head, UndoPtr::ZERO);
    assert_eq!(heap.read_tuple(second_ptr).unwrap().payload, b"second");
}
