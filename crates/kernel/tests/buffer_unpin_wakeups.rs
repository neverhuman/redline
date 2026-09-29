//! Unpinning a page wakes no one.
//!
//! Every `PageGuard` drop called `notify_all` on its frame's condition
//! variable, one futex syscall per unpin, though no waiter waits for a pin
//! count: pins wait for a page to load, writers for a write to finish, and
//! eviction skips a pinned frame rather than waiting for it.

use std::sync::Arc;

use redlinedb_kernel::format::{Lsn, PageKind, RelId};
use redlinedb_kernel::observe;
use redlinedb_kernel::storage::{BufferPool, PageFile};

#[test]
fn pinning_and_unpinning_a_resident_page_wakes_no_one() {
    let dir = tempfile::tempdir().expect("temp dir");
    let file =
        Arc::new(PageFile::create(dir.path().join("data.redline"), 4096).expect("page file"));
    let pool = BufferPool::new(file, 16).expect("buffer pool");
    let page_id = {
        let guard = pool.allocate(PageKind::Heap, RelId(1)).expect("allocate");
        guard.mark_dirty(Lsn(1)).expect("dirty");
        guard.page_id()
    };
    pool.flush_all(Lsn(1)).expect("flush");
    drop(pool.pin(page_id).expect("load"));

    let before = observe::snapshot();
    for _ in 0..100 {
        let guard = pool.pin(page_id).expect("pin");
        guard.with_page(|_| Ok(())).expect("read");
    }
    let woken = observe::snapshot().since(before).frame_wakeups;
    assert_eq!(
        woken, 0,
        "100 pins and unpins of a resident page woke {woken} time(s)"
    );
}
