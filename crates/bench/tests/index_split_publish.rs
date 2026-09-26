//! A crash at `index::split_image` must not publish a leaf split.
//!
//! The split stages both pages, hits this failpoint, and only then
//! appends the page images and installs them. The bench crate builds
//! the kernel with failpoints.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use redlinedb_kernel::failpoints;
use redlinedb_kernel::format::{PageGeneration, PageId, RelId, TxId};
use redlinedb_kernel::index::{BtreeIndex, IndexDescriptor, IndexId, IndexRowRef, IndexUniqueness};
use redlinedb_kernel::storage::{BufferPool, PageFile};
use redlinedb_kernel::wal::{WalConfig, WalCoordinator};

#[test]
fn crash_before_split_image_leaves_prior_keys_in_place() {
    let dir = tempfile::tempdir().expect("temp dir");
    let page_file =
        Arc::new(PageFile::create(dir.path().join("data.redline"), 512).expect("page file"));
    let buffer = Arc::new(BufferPool::new(Arc::clone(&page_file), 64).expect("buffer"));
    let wal = Arc::new(
        WalCoordinator::create(dir.path().join("wal"), WalConfig::default()).expect("wal"),
    );
    let index = BtreeIndex::create_with_wal(
        buffer,
        IndexDescriptor::new(IndexId(3), RelId(1), IndexUniqueness::NonUnique),
        Some(wal),
    )
    .expect("index");
    let row = |slot: u16| {
        IndexRowRef::new(redlinedb_kernel::format::TuplePtr::new_with_generation(
            PageId(100 + u64::from(slot)),
            slot,
            PageGeneration::ONE,
        ))
    };
    failpoints::cfg("index::split_image", "panic").expect("arm failpoint");
    let mut kept = Vec::new();
    let mut crashed = None;
    for i in 0..16_u16 {
        let key = format!("k{i:03}");
        let key_bytes = key.clone().into_bytes();
        let inserted = catch_unwind(AssertUnwindSafe(|| {
            index
                .insert_tx(TxId(1), &key_bytes, row(i))
                .expect("insert");
        }));
        if inserted.is_err() {
            crashed = Some(key);
            break;
        }
        kept.push(key);
    }
    failpoints::cfg("index::split_image", "off").expect("disarm failpoint");
    let crashed = crashed.expect("a leaf split must hit index::split_image");
    assert!(
        index
            .point_lookup(crashed.as_bytes())
            .expect("lookup")
            .is_empty(),
        "the key that split the leaf must not be visible"
    );
    for key in &kept {
        assert_eq!(
            index.point_lookup(key.as_bytes()).expect("lookup").len(),
            1,
            "{key} disappeared"
        );
    }
    assert!(index.validate().expect("validate").errors.is_empty());
}
