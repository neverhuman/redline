//! Pages stay correct and no thread hangs while eight threads pin pages
//! through a pool half the size of the page set, so frames are evicted
//! and reloaded all the time. Unpinning wakes no one; this checks nothing
//! waits on a pin count to change. The pool has room for every thread's
//! pin and a load in flight each, since a pool with no frame to evict
//! refuses a pin rather than waiting.

use std::sync::Arc;

use redlinedb_kernel::format::{Lsn, PageKind, RelId};
use redlinedb_kernel::storage::{BufferPool, PageFile};

const PAGES: usize = 64;
const FRAMES: usize = 32;
const THREADS: usize = 8;
const PINS: usize = 2_000;

#[test]
fn eight_threads_pin_pages_through_a_small_pool() {
    let dir = tempfile::tempdir().expect("temp dir");
    let file =
        Arc::new(PageFile::create(dir.path().join("data.redline"), 4096).expect("page file"));
    let page_ids = {
        let pool = BufferPool::new(Arc::clone(&file), PAGES * 2).expect("loading pool");
        let ids: Vec<_> = (0..PAGES)
            .map(|_| {
                let guard = pool.allocate(PageKind::Heap, RelId(1)).expect("allocate");
                guard.mark_dirty(Lsn(1)).expect("dirty");
                guard.page_id()
            })
            .collect();
        pool.flush_all(Lsn(1)).expect("flush");
        ids
    };
    let pool = Arc::new(BufferPool::new(file, FRAMES).expect("small pool"));
    std::thread::scope(|scope| {
        for thread in 0..THREADS {
            let pool = Arc::clone(&pool);
            let page_ids = &page_ids;
            scope.spawn(move || {
                let mut state = 0x9e37_79b9_u64.wrapping_mul(thread as u64 + 1);
                for _ in 0..PINS {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    let page_id = page_ids[(state % PAGES as u64) as usize];
                    let guard = pool.pin(page_id).expect("pin");
                    let header = guard.with_page(|page| page.header()).expect("header");
                    assert_eq!(header.page_id, page_id, "pinned the wrong page");
                }
            });
        }
    });
    assert!(pool.resident_pages() <= FRAMES);
}
