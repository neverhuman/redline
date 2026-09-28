//! A split that cannot get a page leaves the tree as it was.
//!
//! A split allocates the new right leaf, then a right page for each parent
//! it overflows, then a new root. An allocation fails when the pool has no
//! frame to give. The split used to install the leaf halves before it
//! allocated the parent's pages, so a later failure left a leaf with a right
//! sibling its parent never named, or a root leaf with a sibling and no root
//! above it. The next split under that leaf then treated the leaf as its
//! parent and failed with "expected internal page".

use std::collections::BTreeSet;
use std::sync::Arc;

use super::super::PAGE_LEAF_KIND;
use super::super::cells::Entry;
use crate::Error;
use crate::format::{PageGeneration, PageId, RelId, RowId, TuplePtr};
use crate::index::{BtreeIndex, IndexDescriptor, IndexId, IndexRowRef, IndexUniqueness};
use crate::storage::buffer_test_hooks::fail_allocation_after;
use crate::storage::{BufferPool, PageFile};

/// 200-byte keys fit eight to a 4 KiB leaf and 17 separators to an
/// internal page, so these keys split leaves, parents and the root.
const KEYS: usize = 400;
const FULL_POOL: Error = Error::CorruptPage("no unpinned frame available for eviction");

fn key(i: usize) -> Vec<u8> {
    // Scattered order, so splits land all over the tree.
    let mut key = format!("{:08}", (i * 7919) % KEYS).into_bytes();
    key.resize(200, b'k');
    key
}

fn row(i: usize) -> IndexRowRef {
    IndexRowRef::with_row_id(
        RowId(i as u64 + 1),
        TuplePtr::new_with_generation(PageId(1), (i % 1000) as u16, PageGeneration::ONE),
    )
}

fn header(index: &BtreeIndex, page_id: PageId) -> super::super::PageHeader {
    index
        .inner
        .buffer
        .pin(page_id)
        .unwrap()
        .with_page(BtreeIndex::read_page_header)
        .unwrap()
}

/// Leaves reached from the root through child pointers.
fn leaves_by_descent(index: &BtreeIndex) -> BTreeSet<PageId> {
    let mut leaves = BTreeSet::new();
    let mut pending = vec![index.meta().unwrap().root_page_id];
    while let Some(page_id) = pending.pop() {
        let (page_header, entries) = index
            .inner
            .buffer
            .pin(page_id)
            .unwrap()
            .with_page(|page| {
                Ok((
                    BtreeIndex::read_page_header(page)?,
                    index.read_entries(page)?,
                ))
            })
            .unwrap();
        if page_header.kind == PAGE_LEAF_KIND {
            leaves.insert(page_id);
            continue;
        }
        pending.extend(page_header.left);
        pending.extend(entries.iter().filter_map(|entry| match entry {
            Entry::Internal { child, .. } => Some(*child),
            Entry::Leaf { .. } => None,
        }));
    }
    leaves
}

/// Leaves reached by walking the leaf level from the leftmost leaf through
/// the right-sibling links.
fn leaves_by_sibling_links(index: &BtreeIndex) -> BTreeSet<PageId> {
    let mut page_id = index.meta().unwrap().root_page_id;
    loop {
        let page_header = header(index, page_id);
        if page_header.kind == PAGE_LEAF_KIND {
            break;
        }
        page_id = page_header.left.expect("leftmost child");
    }
    let mut leaves = BTreeSet::new();
    let mut next = Some(page_id);
    while let Some(page_id) = next {
        assert!(leaves.insert(page_id), "sibling links loop at {page_id:?}");
        next = header(index, page_id).right;
    }
    leaves
}

fn assert_whole(index: &BtreeIndex, inserted: usize, context: &str) {
    let report = index.validate().unwrap();
    assert_eq!(report.errors, Vec::<&str>::new(), "{context}");
    assert_eq!(
        leaves_by_descent(index),
        leaves_by_sibling_links(index),
        "{context}: a leaf is reachable only through a sibling link"
    );
    for i in 0..inserted {
        assert_eq!(
            index.point_lookup(&key(i)).unwrap(),
            vec![row(i)],
            "{context}: key {i}"
        );
    }
}

#[test]
fn a_split_that_cannot_allocate_leaves_the_tree_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let file = Arc::new(PageFile::create(temp.path().join("data.redline"), 4096).unwrap());
    let pool = Arc::new(BufferPool::new(file, 4096).unwrap());
    let index = BtreeIndex::create(
        pool,
        IndexDescriptor::new(IndexId(1), RelId(1), IndexUniqueness::NonUnique),
    )
    .unwrap();
    let mut failed_splits = [0_usize; 3];
    for i in 0..KEYS {
        // Fail the first, second or third page the insert asks for. An
        // insert that needs fewer pages goes through on the first try.
        let mut inserted = false;
        for (fail_at, failures) in failed_splits.iter_mut().enumerate() {
            fail_allocation_after(Some(fail_at));
            let result = index.insert(&key(i), row(i));
            fail_allocation_after(None);
            match result {
                Ok(()) => {
                    inserted = true;
                    break;
                }
                Err(err) => {
                    assert_eq!(err, FULL_POOL, "insert {i}");
                    *failures += 1;
                    let context = format!("insert {i} failing allocation {}", fail_at + 1);
                    assert_whole(&index, i, &context);
                    assert_eq!(index.point_lookup(&key(i)).unwrap(), vec![], "{context}");
                }
            }
        }
        if !inserted {
            index
                .insert(&key(i), row(i))
                .unwrap_or_else(|err| panic!("insert {i} after its failures: {err:?}"));
        }
    }
    assert_whole(&index, KEYS, "after every insert");
    // Leaf splits, parent splits and root splits each failed at least once.
    assert!(
        failed_splits.iter().all(|failures| *failures > 0),
        "{failed_splits:?}"
    );
}
