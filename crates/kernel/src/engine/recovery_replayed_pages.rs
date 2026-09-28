//! Heap pages a crashed recovery, or eviction after one, left behind.
//!
//! Heap redo appends each replayed row as a new version on a page it
//! allocates past the end of the page file, and stamps the page with LSN
//! zero, so eviction may write it before any checkpoint covers it. A crash
//! before recovery checkpoints leaves those pages in the file, and the next
//! recovery replays the same records onto new pages again: a page scan would
//! then find each row two or more times.
//!
//! Every heap or undo page past the checkpoint's page count holds only such
//! replay output, or rows a later change appended after the checkpoint
//! wrote its heap pages. The checkpoint writes every dirty page and then
//! reads the page count, so each page that existed when it wrote them lies
//! at or below that count; a page above it was allocated afterwards, and
//! whatever heap records put there are at or past the heap redo LSN, which
//! recovery replays. With no checkpoint, recovery replays the whole WAL. So
//! recovery empties those pages before heap redo and lets redo write the
//! rows once. Index pages are left alone: their redo already skips what a
//! page holds. A page that does not read back whole (a torn write) is left
//! as it was, because its kind cannot be told.

use crate::format::{PageId, PageKind, PageState, RelId};
use crate::storage::{BufferPool, PageFile};
use crate::{Error, Result};

use super::super::page_heap::PageBackedHeap;

/// Empty every heap and undo page of `rel_id` above page `covered` and queue
/// it for reuse. Returns how many pages were emptied. Runs before heap redo
/// pins any of them.
pub(super) fn clear_heap_pages_past_checkpoint(
    page_file: &PageFile,
    buffer: &BufferPool,
    heap: &PageBackedHeap,
    rel_id: RelId,
    covered: u64,
) -> Result<usize> {
    let mut cleared = 0_usize;
    let page_count = page_file.page_count()?;
    for page_no in covered.saturating_add(1)..=page_count {
        let page_id = PageId(page_no);
        let page = match page_file.read_page(page_id) {
            Ok(page) => page,
            Err(Error::InvalidMagic { .. } | Error::InvalidChecksum) => continue,
            Err(err) => return Err(err),
        };
        let header = page.header()?;
        if !matches!(header.kind, PageKind::Heap | PageKind::Undo) || header.rel_id != rel_id {
            continue;
        }
        if header.state == PageState::Reusable && page.slot_count()? == 0 {
            heap.push_reusable_page(header.kind, page_id)?;
            continue;
        }
        let mut empty = page;
        empty.reinitialize(header.kind, page_id, rel_id, header.generation.next())?;
        empty.set_state(PageState::Reusable)?;
        buffer.write_page_direct(&empty)?;
        heap.push_reusable_page(header.kind, page_id)?;
        cleared += 1;
    }
    Ok(cleared)
}

#[cfg(test)]
thread_local! {
    static AFTER_HEAP_REPLAY: std::cell::RefCell<Option<Box<dyn FnMut() -> crate::Result<()>>>> =
        std::cell::RefCell::new(None);
}

/// Test hook that runs on this thread once recovery has replayed the heap. An
/// error stops the open there, as a crash would, with nothing flushed.
#[cfg(test)]
pub(super) fn set_after_heap_replay_hook(hook: Option<Box<dyn FnMut() -> crate::Result<()>>>) {
    AFTER_HEAP_REPLAY.with(|slot| *slot.borrow_mut() = hook);
}

#[cfg(test)]
pub(super) fn run_after_heap_replay_hook() -> crate::Result<()> {
    AFTER_HEAP_REPLAY.with(|slot| match slot.borrow_mut().as_mut() {
        Some(hook) => hook(),
        None => Ok(()),
    })
}
