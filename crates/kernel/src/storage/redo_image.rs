//! Redo of a logged whole-page image.

use crate::format::{Lsn, Page};
use crate::{Error, Result};

use super::BufferPool;

impl BufferPool {
    /// Redo a logged image of a page, whose WAL record ends at `lsn`, unless
    /// the page already holds that change or a later one. Returns whether the
    /// image was applied.
    ///
    /// The page's own LSN decides: the resident frame's, or the file copy's
    /// when the page is not resident (pinning reads it). A page at or past
    /// `lsn` is newer and stays. Otherwise the image, stamped with `lsn`,
    /// replaces the file copy and the resident frame together while the
    /// frame lock is held, so no flush of the older frame can land after it.
    /// The frame then matches the file and is left clean. A page that cannot
    /// be read, torn or past the end of the file, is overwritten.
    ///
    /// Skipping is only sound while every LSN on a page came from this WAL's
    /// LSN sequence, which the reopened WAL keeps by resuming past every
    /// page LSN.
    pub fn redo_page_image(&self, mut image: Page, lsn: Lsn) -> Result<bool> {
        let page_id = image.header()?.page_id;
        image.set_page_lsn(lsn)?;
        let guard = match self.pin(page_id) {
            Ok(guard) => guard,
            Err(Error::Io(err)) if err.kind() != std::io::ErrorKind::UnexpectedEof => {
                return Err(Error::Io(err));
            }
            Err(_) => {
                // Not resident and not loadable: a torn page, a page past
                // the end of the file, or no frame to load it into. Read
                // the file copy directly for its LSN, if it has one.
                let on_disk = self
                    .read_page_bytes_unchecked(page_id)
                    .ok()
                    .and_then(|bytes| Page::from_bytes(bytes).ok());
                if let Some(page) = on_disk
                    && page.header()?.page_lsn >= lsn
                {
                    return Ok(false);
                }
                self.write_page_direct(&image)?;
                return Ok(true);
            }
        };
        let mut frame = guard.mutable_frame()?;
        let resident = frame
            .page
            .as_mut()
            .ok_or(Error::CorruptPage("resident frame missing page"))?;
        if resident.header()?.page_lsn >= lsn {
            return Ok(false);
        }
        self.write_page_direct(&image)?;
        *resident = image;
        frame.dirty = false;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::format::{Lsn, Page, PageId, PageKind, RelId};
    use crate::storage::buffer_test_hooks::page_file_syncs;
    use crate::storage::{BufferPool, PageFile};

    /// Recovery can apply images and then checkpoint with no dirty frame to
    /// flush. That checkpoint records a redo LSN past the images, so it must
    /// still sync the page file they were written to.
    #[test]
    fn the_next_checkpoint_flush_syncs_a_redone_image() {
        let dir = tempfile::tempdir().unwrap();
        let page_file = Arc::new(PageFile::create(dir.path().join("data.redline"), 512).unwrap());
        let buffer = BufferPool::new(Arc::clone(&page_file), 8).unwrap();
        let image = Page::new(512, PageKind::BtreeLeaf, PageId(1), RelId(1)).unwrap();
        assert!(buffer.redo_page_image(image, Lsn(10)).unwrap());

        let before = page_file_syncs();
        let flush = buffer.flush_dirty_batches(Lsn(u64::MAX), 16).unwrap();
        assert_eq!(flush.flushed_pages, 0);
        assert_eq!(page_file_syncs(), before + 1);

        // Once synced, a flush with nothing new to write does not sync again.
        buffer.flush_dirty_batches(Lsn(u64::MAX), 16).unwrap();
        assert_eq!(page_file_syncs(), before + 1);
    }

    #[test]
    fn an_image_at_or_below_the_page_lsn_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let page_file = Arc::new(PageFile::create(dir.path().join("data.redline"), 512).unwrap());
        let buffer = BufferPool::new(Arc::clone(&page_file), 8).unwrap();
        let image = Page::new(512, PageKind::BtreeLeaf, PageId(1), RelId(1)).unwrap();
        assert!(buffer.redo_page_image(image.clone(), Lsn(10)).unwrap());
        assert!(!buffer.redo_page_image(image.clone(), Lsn(10)).unwrap());
        assert!(!buffer.redo_page_image(image.clone(), Lsn(9)).unwrap());
        assert!(buffer.redo_page_image(image, Lsn(11)).unwrap());
        let lsn = page_file
            .read_page(PageId(1))
            .unwrap()
            .header()
            .unwrap()
            .page_lsn;
        assert_eq!(lsn, Lsn(11));
    }
}
