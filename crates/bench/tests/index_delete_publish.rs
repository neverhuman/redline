//! A crash at `index::delete` must not leave the tombstone on the leaf.
//!
//! The delete stages the page privately, hits this failpoint, and only
//! then appends the WAL record and installs the page. The bench crate
//! builds the kernel with failpoints.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use redlinedb_kernel::failpoints;
use redlinedb_kernel::format::{PageGeneration, PageId, RelId, TxId};
use redlinedb_kernel::index::{BtreeIndex, IndexDescriptor, IndexId, IndexRowRef, IndexUniqueness};
use redlinedb_kernel::storage::{BufferPool, PageFile};
use redlinedb_kernel::wal::{WalConfig, WalCoordinator};

#[test]
fn crash_before_delete_wal_leaves_the_key_visible() {
    let dir = tempfile::tempdir().expect("temp dir");
    let page_file =
        Arc::new(PageFile::create(dir.path().join("data.redline"), 4096).expect("page file"));
    let buffer = Arc::new(BufferPool::new(Arc::clone(&page_file), 64).expect("buffer"));
    let wal = Arc::new(
        WalCoordinator::create(dir.path().join("wal"), WalConfig::default()).expect("wal"),
    );
    let index = BtreeIndex::create_with_wal(
        buffer,
        IndexDescriptor::new(IndexId(4), RelId(1), IndexUniqueness::NonUnique),
        Some(wal),
    )
    .expect("index");
    let row = IndexRowRef::new(redlinedb_kernel::format::TuplePtr::new_with_generation(
        PageId(8),
        1,
        PageGeneration::ONE,
    ));
    index.insert_tx(TxId(1), b"kept", row).expect("insert");
    failpoints::cfg("index::delete", "panic").expect("arm failpoint");
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        index.delete_mark_tx(TxId(1), b"kept", row).expect("delete");
    }));
    failpoints::cfg("index::delete", "off").expect("disarm failpoint");
    assert!(panicked.is_err(), "index::delete must panic before publish");
    assert_eq!(
        index.point_lookup(b"kept").expect("lookup").len(),
        1,
        "the live leaf must still contain a key whose delete WAL was not appended"
    );
    index
        .delete_mark_tx(TxId(1), b"kept", row)
        .expect("delete after disarm");
    assert!(
        index.point_lookup(b"kept").expect("lookup").is_empty(),
        "a completed delete marks the key"
    );
}
