#[path = "mutate/delete.rs"]
mod delete;
#[path = "mutate/insert.rs"]
mod insert;
#[path = "mutate/insert_leaf.rs"]
mod insert_leaf;
#[path = "mutate/maintenance.rs"]
mod maintenance;

use crate::format::Lsn;
use crate::storage::buffer::FrameState;
use crate::{Error, Result};

/// The page LSN a committed index change that recovery replays leaves on a
/// leaf it does not split.
///
/// Recovery installs B-tree page images at LSN zero and replays every
/// committed change again after a crash, whatever the page file holds, so
/// such a leaf can reach the file on its own and eviction may write it. A
/// leaf a recovery split already dirtied keeps that split's LSN: it stays in
/// the pool with the split's other pages until a checkpoint writes them all.
pub(super) fn recovered_leaf_lsn(frame: &FrameState) -> Result<Lsn> {
    let page_lsn = frame
        .page
        .as_ref()
        .ok_or(Error::CorruptPage("resident frame missing page"))?
        .header()?
        .page_lsn;
    Ok(if frame.dirty { page_lsn } else { Lsn::ZERO })
}
