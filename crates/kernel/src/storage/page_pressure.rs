//! What a buffer pool asks for when no resident page may leave.

use crate::Result;

/// Eviction writes a dirty page on its own only when recovery can rebuild
/// it alone. When every unpinned frame holds some other dirty page, the pool
/// asks its engine for a checkpoint: that writes the pages as one
/// consistent cut and records where recovery starts, after which they are
/// clean and eviction can drop them.
pub trait PagePressureRelief: Send + Sync {
    /// Run a checkpoint. `Ok(false)` means none ran, and the allocation that
    /// asked fails as it would have without relief.
    fn relieve_page_pressure(&self) -> Result<bool>;
}
