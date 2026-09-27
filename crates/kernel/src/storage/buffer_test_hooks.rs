//! Test hooks for buffer pool page writes and page file syncs.
//!
//! Hooks and counters are per thread. Eviction writes a page on the thread
//! that asked for a frame and a checkpoint syncs on its caller's thread, so a
//! test sees only its own work and none from lib tests running in parallel.

use std::cell::{Cell, RefCell};

use crate::format::{Lsn, PageId};

pub(crate) type PageWriteHook = Box<dyn FnMut(PageId, Lsn)>;

thread_local! {
    static BEFORE_PAGE_WRITE: RefCell<Option<PageWriteHook>> = const { RefCell::new(None) };
    static PAGE_FILE_SYNCS: Cell<u64> = const { Cell::new(0) };
    static FAIL_NEXT_PAGE_FILE_SYNC: Cell<bool> = const { Cell::new(false) };
}

/// Run `hook` on this thread before every buffer pool page write, with the
/// page id and the page LSN of the bytes about to be written. `None` clears it.
pub(crate) fn set_before_page_write_hook(hook: Option<PageWriteHook>) {
    BEFORE_PAGE_WRITE.with(|slot| *slot.borrow_mut() = hook);
}

pub(super) fn run_before_page_write_hook(page_id: PageId, page_lsn: Lsn) {
    BEFORE_PAGE_WRITE.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook(page_id, page_lsn);
        }
    });
}

/// Page file syncs made on this thread.
pub(crate) fn page_file_syncs() -> u64 {
    PAGE_FILE_SYNCS.with(Cell::get)
}

pub(super) fn count_page_file_sync() {
    PAGE_FILE_SYNCS.with(|count| count.set(count.get().saturating_add(1)));
}

/// Make the next page file sync on this thread fail with an I/O error.
pub(crate) fn fail_next_page_file_sync() {
    FAIL_NEXT_PAGE_FILE_SYNC.with(|fail| fail.set(true));
}

pub(super) fn take_page_file_sync_failure() -> bool {
    FAIL_NEXT_PAGE_FILE_SYNC.with(|fail| fail.replace(false))
}
