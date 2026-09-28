//! WAL storage façade.
//!
//! Write-side mutation logic lives in `storage/write.rs`; read-side
//! recovery scanning lives in `storage/scan.rs`; keeping a torn tail's
//! bytes before it is cut off lives in `storage/salvage.rs`.

use super::*;

#[path = "storage/salvage.rs"]
mod salvage;
#[path = "storage/scan.rs"]
mod scan;
#[path = "storage/write.rs"]
mod write;

pub use salvage::WAL_SALVAGE_DIR;
pub(crate) use salvage::{PendingTail, salvage_and_empty_torn_segment};
