//! Index page-image redo against a page that is already resident.

use std::sync::Arc;

use tempfile::TempDir;

use redlinedb_kernel::format::{Lsn, PageGeneration, PageId, RelId, TuplePtr};
use redlinedb_kernel::index::{BtreeIndex, IndexDescriptor, IndexId, IndexRowRef, IndexUniqueness};
use redlinedb_kernel::storage::{BufferPool, PageFile};

const ROOT: PageId = PageId(2);

fn row(page: u64) -> IndexRowRef {
    IndexRowRef::new(TuplePtr::new_with_generation(
        PageId(page),
        1,
        PageGeneration::ONE,
    ))
}

/// A WAL page image stores LSN zero; its order is the record's LSN. When
/// redo applies such an image over a resident page with an older nonzero
/// LSN, the resident frame must take the image too. Otherwise lookups keep
/// reading the stale frame, and a later flush writes it back over the image
/// in the file.
#[test]
fn index_redo_replaces_a_stale_resident_frame_and_its_file_copy() {
    let temp = TempDir::new().unwrap();
    let page_file = Arc::new(PageFile::create(temp.path().join("data.redline"), 512).unwrap());
    let buffer = Arc::new(BufferPool::new(Arc::clone(&page_file), 64).unwrap());
    let index = BtreeIndex::create(
        Arc::clone(&buffer),
        IndexDescriptor::new(IndexId(21), RelId(1), IndexUniqueness::NonUnique),
    )
    .unwrap();
    index.insert(b"kept", row(40)).unwrap();
    let mut image = buffer
        .pin(ROOT)
        .unwrap()
        .with_page(|page| Ok(page.clone()))
        .unwrap();
    image.set_page_lsn(Lsn::ZERO).unwrap();

    // The resident root then holds a key the image lacks, at LSN 1, dirty.
    index.insert(b"stale", row(41)).unwrap();
    let resident_lsn = buffer
        .pin(ROOT)
        .unwrap()
        .with_page(|page| Ok(page.header()?.page_lsn))
        .unwrap();
    assert!(resident_lsn > Lsn::ZERO);

    index
        .redo_page_image(image, Lsn(resident_lsn.get() + 1))
        .unwrap();
    assert_eq!(index.point_lookup(b"kept").unwrap().len(), 1);
    assert!(
        index.point_lookup(b"stale").unwrap().is_empty(),
        "the resident frame kept the stale page"
    );

    buffer.flush_all(Lsn(u64::MAX)).unwrap();
    drop(index);
    drop(buffer);
    let fresh = Arc::new(BufferPool::new(Arc::clone(&page_file), 64).unwrap());
    let reopened = BtreeIndex::open(
        Arc::clone(&fresh),
        PageId(1),
        IndexDescriptor::new(IndexId(21), RelId(1), IndexUniqueness::NonUnique),
    )
    .unwrap();
    assert_eq!(reopened.point_lookup(b"kept").unwrap().len(), 1);
    assert!(
        reopened.point_lookup(b"stale").unwrap().is_empty(),
        "a flush wrote the stale frame back over the image"
    );
}
