//! The checkpoint flush: every dirty page, whatever its LSN (workplan R5).
//!
//! A checkpoint records an LSN below which recovery replays nothing and the
//! WAL may be pruned, so every change below that LSN has to reach the page
//! file. A page another writer changed after the checkpoint chose its LSN
//! still holds the older changes. The batch flush skips such a page whole,
//! which lost them; this flush writes it as it stands, after making the WAL
//! durable through its page LSN.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{BufferPool, FlushStats, FrameEntry, Inner};
use crate::format::Lsn;
use crate::{Error, Result};

impl BufferPool {
    /// Write every dirty page and sync the page file, for a checkpoint whose
    /// WAL is durable through `durable`.
    ///
    /// No dirty page is skipped for an LSN past `durable`. Before writing
    /// such a page, `force_wal` makes the WAL durable through the page LSN it
    /// is given and returns the durable LSN. It runs under the page's frame
    /// lock, in the frame-then-WAL order a heap append takes them. A page LSN
    /// past every appended record is a placeholder, not a record; `force_wal`
    /// may return less than it.
    pub fn flush_dirty_for_checkpoint(
        &self,
        durable: Lsn,
        force_wal: &dyn Fn(Lsn) -> Result<Lsn>,
    ) -> Result<FlushStats> {
        let stats = self.write_dirty_for_checkpoint(durable, force_wal)?;
        self.sync_checkpoint_writes(stats)?;
        Ok(stats)
    }

    /// The writes of [`Self::flush_dirty_for_checkpoint`], without the sync.
    pub(crate) fn write_dirty_for_checkpoint(
        &self,
        durable: Lsn,
        force_wal: &dyn Fn(Lsn) -> Result<Lsn>,
    ) -> Result<FlushStats> {
        self.inner.write_dirty_for_checkpoint(durable, force_wal)
    }

    /// Sync the page file after [`Self::write_dirty_for_checkpoint`], and
    /// after any eviction write since the last sync.
    pub(crate) fn sync_checkpoint_writes(&self, stats: FlushStats) -> Result<()> {
        self.inner.sync_flushed_pages(stats.flushed_pages > 0)
    }
}

impl Inner {
    fn write_dirty_for_checkpoint(
        &self,
        mut durable: Lsn,
        force_wal: &dyn Fn(Lsn) -> Result<Lsn>,
    ) -> Result<FlushStats> {
        let (frames, newest) = self.every_dirty_frame()?;
        // One WAL flush up front covers every page as it stands now. A page
        // that changes before its turn is forced again under its frame lock.
        if newest > durable {
            durable = durable.max(force_wal(newest)?);
        }
        let mut flushed_pages = 0_usize;
        for frame in frames {
            if self.write_frame_for_checkpoint(&frame, &mut durable, force_wal)? {
                flushed_pages += 1;
                self.stats
                    .checkpoint_flushes
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok(FlushStats {
            flushed_pages,
            batches: usize::from(flushed_pages > 0),
        })
    }

    /// Every dirty frame, with the highest page LSN among them. A frame an
    /// eviction or another flush is writing is included: its write may start
    /// before a later change, and the checkpoint waits for it and looks again.
    fn every_dirty_frame(&self) -> Result<(Vec<Arc<FrameEntry>>, Lsn)> {
        let mut frames = Vec::new();
        let mut newest = Lsn::ZERO;
        for shard in &self.shards {
            let shard = shard
                .lock()
                .map_err(|_| Error::CorruptPage("buffer shard poisoned"))?;
            for frame in shard.values() {
                let state = frame
                    .state
                    .lock()
                    .map_err(|_| Error::CorruptPage("buffer frame poisoned"))?;
                if !state.dirty {
                    continue;
                }
                if let Some(page) = state.page.as_ref() {
                    newest = newest.max(page.header()?.page_lsn);
                }
                frames.push(Arc::clone(frame));
            }
        }
        Ok((frames, newest))
    }

    fn write_frame_for_checkpoint(
        &self,
        frame: &Arc<FrameEntry>,
        durable: &mut Lsn,
        force_wal: &dyn Fn(Lsn) -> Result<Lsn>,
    ) -> Result<bool> {
        let (page, written_lsn) = {
            let mut state = frame
                .state
                .lock()
                .map_err(|_| Error::CorruptPage("buffer frame poisoned"))?;
            while state.dirty && (state.write_in_progress || state.page.is_none()) {
                state = frame
                    .ready
                    .wait(state)
                    .map_err(|_| Error::CorruptPage("buffer frame wait poisoned"))?;
            }
            if !state.dirty {
                return Ok(false);
            }
            let page = state
                .page
                .as_ref()
                .ok_or_else(|| Error::CorruptPage("resident frame missing page"))?;
            let page_lsn = page.header()?.page_lsn;
            if page_lsn > *durable {
                *durable = (*durable).max(force_wal(page_lsn)?);
            }
            let page = page.clone();
            state.write_in_progress = true;
            (page, page_lsn)
        };

        #[cfg(test)]
        super::super::buffer_test_hooks::run_before_page_write_hook(
            page.header()?.page_id,
            written_lsn,
        );
        let write_result = self.page_file.write_page(&page);
        let mut state = frame
            .state
            .lock()
            .map_err(|_| Error::CorruptPage("buffer frame poisoned"))?;
        state.write_in_progress = false;
        crate::observe::add_frame_wakeup();
        crate::observe::add_frame_notify();
        frame.ready.notify_all();
        write_result?;
        // Nothing could change the page while it was being written: a writer
        // waits for `write_in_progress`. The LSN check is a second guard.
        let current_lsn = state
            .page
            .as_ref()
            .ok_or_else(|| Error::CorruptPage("resident frame missing page"))?
            .header()?
            .page_lsn;
        if current_lsn <= written_lsn {
            state.dirty = false;
        }
        self.stats.writes.fetch_add(1, Ordering::Relaxed);
        Ok(true)
    }
}
