pub mod buffer;
#[cfg(test)]
pub(crate) mod buffer_test_hooks;
pub mod control;
pub mod numa;
pub mod page_file;
pub mod page_wal;
pub(crate) mod policy;
pub mod tx_status_checkpoint;

pub use buffer::*;
pub use control::*;
pub use page_file::*;
pub use page_wal::*;
pub use tx_status_checkpoint::*;

use std::fs::File;
use std::path::Path;

use crate::Result;

/// Open the directory at `path` and `sync_data` it so a preceding rename
/// or create-and-close becomes durable on POSIX. Shared by every storage
/// submodule that touches the database directory atomically; prior to the
/// dedup pass each submodule carried its own private copy of this body.
#[inline]
pub(crate) fn sync_parent_dir(path: &Path) -> Result<()> {
    let file = File::open(path)?;
    file.sync_data()?;
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
