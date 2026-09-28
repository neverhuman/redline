pub mod buffer;
#[cfg(test)]
pub(crate) mod buffer_test_hooks;
pub mod control;
pub mod numa;
pub mod page_file;
pub mod page_pressure;
pub(crate) mod policy;
pub mod tx_status_checkpoint;

pub use buffer::*;
pub use control::*;
pub use page_file::*;
pub use page_pressure::*;
pub use tx_status_checkpoint::*;

use std::path::Path;

use crate::Result;
use crate::io::{FileSystem, StdFileSystem};

/// Fsync the directory at `path` so a preceding rename or create-and-close
/// becomes durable on POSIX. Shared by every storage submodule that touches
/// the database directory atomically; it is [`FileSystem::sync_dir`] on the
/// standard file system.
#[inline]
pub(crate) fn sync_parent_dir(path: &Path) -> Result<()> {
    StdFileSystem.sync_dir(path)?;
    #[cfg(test)]
    PARENT_DIR_SYNCS.with(|count| count.set(count.get().saturating_add(1)));
    Ok(())
}

#[cfg(test)]
thread_local! {
    static PARENT_DIR_SYNCS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn take_parent_dir_syncs() -> u64 {
    PARENT_DIR_SYNCS.with(|count| count.replace(0))
}
