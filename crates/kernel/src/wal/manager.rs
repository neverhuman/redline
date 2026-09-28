//! WAL manager façade.
//!
//! Configuration, counters, and shared types live in `manager/config.rs`,
//! `manager/counters.rs`, and `manager/types.rs`. The operational logic stays
//! in `manager/coordinator.rs` and `manager/storage.rs`.

#[path = "manager/config.rs"]
mod config;
mod coordinator;
#[path = "manager/counters.rs"]
mod counters;
mod storage;
#[path = "manager/types.rs"]
mod types;

pub use config::*;
pub(crate) use coordinator::PageInstallFence;
pub use counters::*;
pub use storage::WAL_SALVAGE_DIR;
pub(crate) use storage::{PendingTail, salvage_and_empty_torn_segment};
pub use types::*;
