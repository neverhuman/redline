//! Test hooks for heap page writes.

use crate::format::Page;

pub(crate) type PageHook = Box<dyn FnMut(&Page)>;

thread_local! {
    static BEFORE_HEAP_INSTALL: std::cell::RefCell<Option<PageHook>> =
        std::cell::RefCell::new(None);
    static BEFORE_TUPLE_OVERWRITE: std::cell::RefCell<Option<PageHook>> =
        std::cell::RefCell::new(None);
}

/// Run `hook` once on this thread, after the next heap append has written its
/// WAL record and before it installs the staged page. The hook sees the
/// resident page as it is at that moment.
pub(crate) fn set_before_heap_install_hook(hook: Option<PageHook>) {
    BEFORE_HEAP_INSTALL.with(|slot| *slot.borrow_mut() = hook);
}

pub(super) fn run_before_heap_install_hook(resident: &Page) {
    let hook = BEFORE_HEAP_INSTALL.with(|slot| slot.borrow_mut().take());
    if let Some(mut hook) = hook {
        hook(resident);
    }
}

/// Run `hook` once on this thread, after the next in-place tuple overwrite has
/// read the resident page and before it writes the tuple.
pub(crate) fn set_before_tuple_overwrite_hook(hook: Option<PageHook>) {
    BEFORE_TUPLE_OVERWRITE.with(|slot| *slot.borrow_mut() = hook);
}

pub(super) fn run_before_tuple_overwrite_hook(resident: &Page) {
    let hook = BEFORE_TUPLE_OVERWRITE.with(|slot| slot.borrow_mut().take());
    if let Some(mut hook) = hook {
        hook(resident);
    }
}
