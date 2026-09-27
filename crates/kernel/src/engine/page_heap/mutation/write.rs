use super::{PageBackedHeap, RelationWriteTarget};

#[path = "write/append.rs"]
mod append;
#[cfg(test)]
#[path = "write/append_tests.rs"]
mod append_tests;
#[cfg(test)]
#[path = "write/install_hook.rs"]
mod install_hook;
#[path = "write/ops.rs"]
mod ops;
#[path = "write/recovery.rs"]
mod recovery;
