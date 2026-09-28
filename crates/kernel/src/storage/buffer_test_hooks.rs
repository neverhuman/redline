//! Test hooks for buffer pool pins, page writes and page file syncs.
//!
//! Hooks and counters are per thread. Eviction writes a page on the thread
//! that asked for a frame and a checkpoint syncs on its caller's thread, so a
//! test sees only its own work and none from lib tests running in parallel.

use std::cell::{Cell, RefCell};

use crate::format::{Lsn, PageId};

pub(crate) type PageWriteHook = Box<dyn FnMut(PageId, Lsn)>;
pub(crate) type PinHook = Box<dyn FnMut(PageId)>;

thread_local! {
    static BEFORE_PAGE_WRITE: RefCell<Option<PageWriteHook>> = const { RefCell::new(None) };
    static PAGE_FILE_SYNCS: Cell<u64> = const { Cell::new(0) };
    static FAIL_NEXT_PAGE_FILE_SYNC: Cell<bool> = const { Cell::new(false) };
    static BEFORE_PIN_LOCK: RefCell<Option<PinHook>> = const { RefCell::new(None) };
    static ALLOCATIONS_BEFORE_FAILURE: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Make the allocation after the next `allocations` on this thread fail as
/// a full pool does. `None` disarms it.
pub(crate) fn fail_allocation_after(allocations: Option<usize>) {
    ALLOCATIONS_BEFORE_FAILURE.with(|slot| slot.set(allocations));
}

pub(super) fn take_allocation_failure() -> crate::Result<()> {
    ALLOCATIONS_BEFORE_FAILURE.with(|slot| match slot.get() {
        Some(0) => {
            slot.set(None);
            Err(crate::Error::CorruptPage(
                "no unpinned frame available for eviction",
            ))
        }
        Some(left) => {
            slot.set(Some(left - 1));
            Ok(())
        }
        None => Ok(()),
    })
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

/// Run `hook` once on this thread, when the next pin of a resident page has
/// found its frame and not yet locked it.
pub(crate) fn set_before_pin_lock_hook(hook: Option<PinHook>) {
    BEFORE_PIN_LOCK.with(|slot| *slot.borrow_mut() = hook);
}

pub(super) fn run_before_pin_lock_hook(page_id: PageId) {
    let hook = BEFORE_PIN_LOCK.with(|slot| slot.borrow_mut().take());
    if let Some(mut hook) = hook {
        hook(page_id);
    }
}
