//! WAL coordinator façade.
//!
//! The implementation lives in `coordinator/methods.rs`,
//! `coordinator/helpers.rs`, and `coordinator/writer.rs`.

use super::*;

#[path = "coordinator/control.rs"]
mod control;
#[cfg(test)]
#[path = "coordinator/failure_tests.rs"]
mod failure_tests;
#[path = "coordinator/helpers.rs"]
mod helpers;
#[path = "coordinator/methods.rs"]
mod methods;
#[path = "coordinator/writer.rs"]
mod writer;

pub(crate) use control::PageInstallFence;
