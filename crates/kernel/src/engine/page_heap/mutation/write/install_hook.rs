//! Test hook for the heap append path.

use crate::format::Page;

type InstallHook = Box<dyn FnMut(&Page)>;

thread_local! {
    static BEFORE_HEAP_INSTALL: std::cell::RefCell<Option<InstallHook>> =
        std::cell::RefCell::new(None);
}

/// Run `hook` once on this thread, after the next heap append has written its
/// WAL record and before it installs the staged page. The hook sees the
/// resident page as it is at that moment.
pub(crate) fn set_before_heap_install_hook(hook: Option<InstallHook>) {
    BEFORE_HEAP_INSTALL.with(|slot| *slot.borrow_mut() = hook);
}

pub(super) fn run_before_heap_install_hook(resident: &Page) {
    let hook = BEFORE_HEAP_INSTALL.with(|slot| slot.borrow_mut().take());
    if let Some(mut hook) = hook {
        hook(resident);
    }
}
