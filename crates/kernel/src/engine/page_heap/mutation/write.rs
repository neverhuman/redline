use super::{PageBackedHeap, RelationWriteTarget};

#[path = "write/append.rs"]
mod append;
#[cfg(test)]
#[path = "write/append_tests.rs"]
mod append_tests;
#[path = "write/ops.rs"]
mod ops;
#[path = "write/recovery.rs"]
mod recovery;
