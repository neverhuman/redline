//! `recover_index_page_images` redoes an image only over an older page.

use std::sync::Arc;

use tempfile::TempDir;

use super::{RecoveryTarget, recover_index_page_images};
use crate::format::{Lsn, Page, PageGeneration, PageId, RelId, TuplePtr, TxId};
use crate::index::{BtreeIndex, IndexDescriptor, IndexId, IndexRowRef, IndexUniqueness};
use crate::storage::{BufferPool, PageFile};
use crate::wal::{WalPayload, WalRecord, WalRecordKind};

const ROOT: PageId = PageId(2);

fn row(page: u64) -> IndexRowRef {
    IndexRowRef::new(TuplePtr::new_with_generation(
        PageId(page),
        1,
        PageGeneration::ONE,
    ))
}

fn descriptor() -> IndexDescriptor {
    IndexDescriptor::new(IndexId(31), RelId(1), IndexUniqueness::NonUnique)
}

/// A logged image as the WAL stores it: page LSN zero, order in `lsn`.
fn image_record(lsn: u64, page: &Page) -> WalRecord {
    let mut image = page.clone();
    image.set_page_lsn(Lsn::ZERO).unwrap();
    WalRecord {
        lsn: Lsn(lsn),
        prev_lsn: Lsn::ZERO,
        tx_id: TxId(7),
        kind: WalRecordKind::PageImage,
        payload: WalPayload::PageImage {
            page_id: ROOT,
            page_lsn: Lsn::ZERO,
            page_bytes: image.as_bytes().to_vec(),
        }
        .encode()
        .unwrap(),
    }
}

fn root_page(buffer: &BufferPool) -> Page {
    buffer
        .pin(ROOT)
        .unwrap()
        .with_page(|page| Ok(page.clone()))
        .unwrap()
}

/// Build a root holding "old", capture it, add "new", and leave the file
/// copy of the root at `file_lsn` holding both keys. Returns the page file
/// and the captured older root.
fn file_with_newer_root(dir: &TempDir, file_lsn: Lsn) -> (Arc<PageFile>, Page) {
    let page_file = Arc::new(PageFile::create(dir.path().join("data.redline"), 512).unwrap());
    let buffer = Arc::new(BufferPool::new(Arc::clone(&page_file), 64).unwrap());
    let index = BtreeIndex::create(Arc::clone(&buffer), descriptor()).unwrap();
    index.insert(b"old", row(40)).unwrap();
    let older = root_page(&buffer);
    index.insert(b"new", row(41)).unwrap();
    buffer.pin(ROOT).unwrap().mark_dirty(file_lsn).unwrap();
    buffer.flush_all(Lsn(u64::MAX)).unwrap();
    (page_file, older)
}

fn keys_in_file(page_file: &Arc<PageFile>) -> (usize, usize) {
    let buffer = Arc::new(BufferPool::new(Arc::clone(page_file), 64).unwrap());
    let index = BtreeIndex::open(buffer, PageId(1), descriptor()).unwrap();
    (
        index.point_lookup(b"old").unwrap().len(),
        index.point_lookup(b"new").unwrap().len(),
    )
}

#[test]
fn an_index_image_older_than_the_file_page_is_skipped() {
    let dir = TempDir::new().unwrap();
    let (page_file, older) = file_with_newer_root(&dir, Lsn(10_000));
    let buffer = Arc::new(BufferPool::new(Arc::clone(&page_file), 64).unwrap());
    let records = vec![image_record(100, &older)];

    recover_index_page_images(&records, Lsn::ZERO, RecoveryTarget::Latest, &buffer).unwrap();

    let resident = root_page(&buffer);
    assert_eq!(resident.header().unwrap().page_lsn, Lsn(10_000));
    drop(buffer);
    assert_eq!(
        keys_in_file(&page_file),
        (1, 1),
        "an older image replaced the newer page"
    );
}

#[test]
fn an_index_image_newer_than_the_file_page_replaces_it() {
    let dir = TempDir::new().unwrap();
    let (page_file, older) = file_with_newer_root(&dir, Lsn(10));
    let buffer = Arc::new(BufferPool::new(Arc::clone(&page_file), 64).unwrap());
    let record = image_record(100, &older);
    let end = Lsn(record.lsn.0 + record.encoded_len() as u64);

    recover_index_page_images(&[record], Lsn::ZERO, RecoveryTarget::Latest, &buffer).unwrap();

    // The resident page and the file copy both carry the image, stamped
    // with the end of its record.
    assert_eq!(root_page(&buffer).header().unwrap().page_lsn, end);
    drop(buffer);
    assert_eq!(keys_in_file(&page_file), (1, 0));
    assert_eq!(
        page_file
            .read_page(ROOT)
            .unwrap()
            .header()
            .unwrap()
            .page_lsn,
        end
    );
}
