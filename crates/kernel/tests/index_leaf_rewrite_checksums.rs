//! Changing one entry of a B-tree leaf checksums the leaf a few times, not
//! once per entry it holds.
//!
//! A delete-mark (every UPDATE of an indexed column and every DELETE makes
//! one) rewrites its leaf, as an insert that cannot take the direct path
//! does: the page is reinitialised and every entry is written back. Every
//! cell written refreshed the whole page's checksum, so a change to a leaf
//! of N entries computed N checksums of the page; an UPDATE of an indexed
//! column checksummed about 2 MB. The rewrite now refreshes the checksum
//! once, after the last cell.

use std::sync::Arc;

use redlinedb_kernel::format::{DEFAULT_PAGE_SIZE, PageGeneration, PageId, RelId, TuplePtr};
use redlinedb_kernel::index::{BtreeIndex, IndexDescriptor, IndexId, IndexRowRef, IndexUniqueness};
use redlinedb_kernel::observe;
use redlinedb_kernel::storage::{BufferPool, PageFile};

/// Entries in the leaf when one of them is delete-marked.
const ENTRIES: u64 = 60;

fn row(n: u64) -> IndexRowRef {
    IndexRowRef::new(TuplePtr::new_with_generation(
        PageId(1_000 + n),
        1,
        PageGeneration::ONE,
    ))
}

#[test]
fn delete_marking_one_entry_checksums_the_leaf_a_few_times() {
    let dir = tempfile::tempdir().expect("temp dir");
    let page_file = Arc::new(
        PageFile::create(dir.path().join("data.redline"), DEFAULT_PAGE_SIZE).expect("page file"),
    );
    let buffer = Arc::new(BufferPool::new(page_file, 64).expect("buffer pool"));
    let index = BtreeIndex::create(
        buffer,
        IndexDescriptor::new(IndexId(7), RelId(1), IndexUniqueness::NonUnique),
    )
    .expect("index");
    for n in 0..ENTRIES {
        index
            .insert(format!("key {n:04}").as_bytes(), row(n))
            .expect("insert");
    }
    let before = observe::snapshot();
    index
        .delete_mark(b"key 0030", row(30))
        .expect("delete-mark one entry");
    let checksummed = observe::snapshot().since(before).checksum_bytes;
    let pages = checksummed / DEFAULT_PAGE_SIZE as u64;
    assert!(
        pages <= 8,
        "delete-marking one entry of a leaf of {ENTRIES} checksummed {pages} pages ({checksummed} bytes)"
    );
    assert_eq!(index.point_lookup(b"key 0031").expect("lookup").len(), 1);
}
