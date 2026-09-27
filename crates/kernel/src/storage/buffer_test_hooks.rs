//! Test hooks for buffer pool page writes.
//!
//! Hooks are per thread. Eviction writes a page on the thread that asked for
//! a frame, so a test sees the writes its own inserts cause and none from
//! lib tests running in parallel.

use std::cell::RefCell;

use crate::format::{Lsn, PageId};

pub(crate) type PageWriteHook = Box<dyn FnMut(PageId, Lsn)>;

thread_local! {
    static BEFORE_PAGE_WRITE: RefCell<Option<PageWriteHook>> = const { RefCell::new(None) };
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
