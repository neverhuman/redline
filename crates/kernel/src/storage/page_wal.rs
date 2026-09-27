//! The WAL a buffer pool flushes before it writes a dirty page.

use crate::Result;
use crate::format::Lsn;

/// A dirty page may reach the page file only after every WAL record up to
/// its page LSN is durable. Otherwise a crash can keep the page and lose the
/// record that explains it. The engine attaches its WAL to the buffer pool,
/// and eviction calls this before it writes a page out.
pub trait PageWal: Send + Sync {
    /// Make the WAL durable through `page_lsn` and return the durable LSN.
    ///
    /// A result below `page_lsn` means the WAL holds no record that far, so
    /// the page must stay in memory.
    fn make_durable_for_page(&self, page_lsn: Lsn) -> Result<Lsn>;
}
