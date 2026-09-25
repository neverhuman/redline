//! A crash at `index::insert` must not leave the new key on the leaf.
//!
//! The insert stages the page privately, hits this failpoint, and only
//! then appends the WAL record and installs the page. The bench crate
//! builds the kernel with failpoints, so this runs in `tests (bench)`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use redlinedb_kernel::failpoints;
use redlinedb_kernel::format::{PageGeneration, PageId, TuplePtr, TxId};
use redlinedb_kernel::index::{BtreeIndex, IndexDescriptor, IndexId, IndexRowRef, IndexUniqueness};
use redlinedb_kernel::storage::{BufferPool, PageFile};

#[test]
fn crash_before_wal_leaves_the_leaf_without_the_key() {
    let dir = tempfile::tempdir().expect("temp dir");
    let page_file =
        Arc::new(PageFile::create(dir.path().join("data.redline"), 4096).expect("page file"));
    let buffer = Arc::new(BufferPool::new(Arc::clone(&page_file), 64).expect("buffer"));
    let index = BtreeIndex::create(
        buffer,
        IndexDescriptor::new(
            IndexId(9),
            redlinedb_kernel::format::RelId(1),
            IndexUniqueness::NonUnique,
        ),
    )
    .expect("index");
    let row = IndexRowRef::new(TuplePtr::new_with_generation(
        PageId(3),
        1,
        PageGeneration::ONE,
    ));
    failpoints::cfg("index::insert", "panic").expect("arm failpoint");
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        index.insert_tx(TxId(1), b"staged", row).expect("insert");
    }));
    failpoints::cfg("index::insert", "off").expect("disarm failpoint");
    assert!(panicked.is_err(), "index::insert must panic before publish");
    assert!(
        index.point_lookup(b"staged").expect("lookup").is_empty(),
        "the live leaf must not contain a key whose WAL record was not appended"
    );
}
