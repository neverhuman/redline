use std::sync::Arc;

use tempfile::TempDir;

use redlinedb_kernel::format::{PageGeneration, PageId, RelId, TuplePtr};
use redlinedb_kernel::index::{BtreeIndex, IndexDescriptor, IndexId, IndexRowRef, IndexUniqueness};
use redlinedb_kernel::storage::{BufferPool, PageFile};

fn row(slot: u16) -> IndexRowRef {
    IndexRowRef::new(TuplePtr::new_with_generation(
        PageId(40),
        slot,
        PageGeneration::ONE,
    ))
}

#[test]
fn inserting_the_same_row_twice_keeps_one_entry() {
    let temp = TempDir::new().unwrap();
    let page_file = Arc::new(PageFile::create(temp.path().join("data.redline"), 512).unwrap());
    let buffer = Arc::new(BufferPool::new(page_file, 32).unwrap());
    let index = BtreeIndex::create(
        buffer,
        IndexDescriptor::new(IndexId(9), RelId(1), IndexUniqueness::NonUnique),
    )
    .unwrap();
    index.insert(b"k", row(1)).unwrap();
    index.insert(b"k", row(1)).unwrap();
    assert_eq!(index.point_lookup(b"k").unwrap().len(), 1);
    index.insert(b"k", row(2)).unwrap();
    assert_eq!(index.point_lookup(b"k").unwrap().len(), 2);
}
