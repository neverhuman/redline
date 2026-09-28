//! The heap side of a checkpoint's complete cut (workplan R5).
//!
//! Heap redo is logical: recovery appends a replayed row as a new version on
//! a new page, not onto the page the record changed, so it cannot tell
//! whether the page file already holds a record's row. A checkpoint
//! therefore writes the heap pages at an instant when no logged heap change
//! is between its WAL append and its page install, and records the WAL
//! position of that instant as the heap redo LSN. The pages then hold every
//! heap record below that LSN and none at or above it.
//!
//! Each logged heap append holds this gate shared from before it locks its
//! frame until its page is installed. The checkpoint holds it exclusively
//! while it writes the pages. Only a frame lock and the WAL append are taken
//! under the shared gate, so a checkpoint waiting for it waits only for
//! appends already past their allocation.

use std::cell::Cell;
use std::sync::{RwLockReadGuard, RwLockWriteGuard};

use super::PageBackedHeap;
use crate::{Error, Result};

thread_local! {
    /// Logged heap changes this thread is inside. A checkpoint started on a
    /// thread that holds the gate shared would wait for itself forever.
    static INSIDE_LOGGED_CHANGE: Cell<usize> = const { Cell::new(0) };
}

/// A logged heap change in flight; see the module docs.
pub(crate) struct LoggedHeapChange<'a> {
    _shared: RwLockReadGuard<'a, ()>,
}

impl Drop for LoggedHeapChange<'_> {
    fn drop(&mut self) {
        INSIDE_LOGGED_CHANGE.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

impl PageBackedHeap {
    /// Enter a heap change that appends a WAL record and installs its page.
    pub(crate) fn begin_logged_change(&self) -> Result<LoggedHeapChange<'_>> {
        let shared = self
            .logged_changes
            .read()
            .map_err(|_| Error::CorruptPage("heap checkpoint gate poisoned"))?;
        INSIDE_LOGGED_CHANGE.with(|depth| depth.set(depth.get().saturating_add(1)));
        Ok(LoggedHeapChange { _shared: shared })
    }

    /// Wait for every logged heap change in flight to install its page, and
    /// hold off new ones until the guard drops.
    pub(crate) fn quiesce_logged_changes(&self) -> Result<RwLockWriteGuard<'_, ()>> {
        if INSIDE_LOGGED_CHANGE.with(Cell::get) > 0 {
            return Err(Error::CorruptPage(
                "checkpoint started inside a logged heap change",
            ));
        }
        self.logged_changes
            .write()
            .map_err(|_| Error::CorruptPage("heap checkpoint gate poisoned"))
    }
}
