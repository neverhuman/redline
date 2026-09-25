//! One-cell leaf insert. On unless `REDLINEDB_LEAF_DIRECT_INSERT=0`.
//!
//! A non-splitting insert places one cell and checksums the page once.
//! The full-page rebuild remains for splits and for the env override.

use std::cell::Cell;

use crate::format::Page;
use crate::{Error, Result};

use super::super::cells::{Entry, LeafCell};

thread_local! {
    static FORCE_DIRECT: Cell<Option<bool>> = const { Cell::new(None) };
}

pub(super) fn direct_leaf_insert_enabled() -> bool {
    if let Some(forced) = FORCE_DIRECT.with(Cell::get) {
        return forced;
    }
    std::env::var("REDLINEDB_LEAF_DIRECT_INSERT")
        .ok()
        .as_deref()
        != Some("0")
}

pub(super) fn insert_one_cell(page: &mut Page, slot: usize, entry: &Entry) -> Result<()> {
    let Entry::Leaf {
        logical_key,
        row,
        physical,
        create_tx,
        delete_tx,
    } = entry
    else {
        return Err(Error::CorruptPage(
            "direct leaf insert expected a leaf cell",
        ));
    };
    let slot = u16::try_from(slot).map_err(|_| Error::CorruptPage("leaf slot overflow"))?;
    let encoded = LeafCell::encode(logical_key, *row, physical, *create_tx, *delete_tx);
    page.insert_cell_at(slot, &encoded)?;
    Ok(())
}

#[cfg(test)]
fn force_direct(value: Option<bool>) {
    FORCE_DIRECT.with(|cell| cell.set(value));
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::format::{PageGeneration, PageId, TuplePtr};
    use crate::index::{BtreeIndex, IndexDescriptor, IndexId, IndexRowRef, IndexUniqueness};
    use crate::observe;
    use crate::storage::{BufferPool, PageFile};
    use crate::{Error, Result};

    use super::force_direct;

    struct ForceGuard;

    impl ForceGuard {
        fn set(value: bool) -> Self {
            force_direct(Some(value));
            Self
        }
    }

    impl Drop for ForceGuard {
        fn drop(&mut self) {
            force_direct(None);
        }
    }

    fn row(page: u64, slot: u16) -> IndexRowRef {
        IndexRowRef::new(TuplePtr::new_with_generation(
            PageId(page),
            slot,
            PageGeneration::ONE,
        ))
    }

    fn fresh(unique: IndexUniqueness) -> Result<(tempfile::TempDir, BtreeIndex)> {
        let dir = tempfile::tempdir().expect("temp dir");
        let page_file = Arc::new(PageFile::create(dir.path().join("data.redline"), 4096)?);
        let buffer = Arc::new(BufferPool::new(page_file, 64)?);
        let index = BtreeIndex::create(
            buffer,
            IndexDescriptor::new(IndexId(7), crate::format::RelId(1), unique),
        )?;
        Ok((dir, index))
    }

    fn fill(index: &BtreeIndex, count: u64) -> Result<()> {
        for i in 0..count {
            let key = format!("k{i:04}");
            index.insert(key.as_bytes(), row(100 + i, i as u16))?;
        }
        Ok(())
    }

    #[test]
    fn default_direct_insert_is_on_unless_zero() {
        force_direct(None);
        let disabled = std::env::var("REDLINEDB_LEAF_DIRECT_INSERT")
            .ok()
            .as_deref()
            == Some("0");
        assert_eq!(super::direct_leaf_insert_enabled(), !disabled);
    }

    #[test]
    fn direct_insert_matches_rewrite_and_skips_rebuild() {
        let _off = ForceGuard::set(false);
        let (_rewrite_dir, rewrite) = fresh(IndexUniqueness::NonUnique).expect("rewrite index");
        let before_rewrite = observe::snapshot();
        fill(&rewrite, 32).expect("rewrite fill");
        let rewrite_calls = observe::snapshot().since(before_rewrite).rewrite_leaf_calls;
        drop(_off);

        let _on = ForceGuard::set(true);
        let (_direct_dir, direct) = fresh(IndexUniqueness::NonUnique).expect("direct index");
        let before_direct = observe::snapshot();
        fill(&direct, 32).expect("direct fill");
        let direct_calls = observe::snapshot().since(before_direct).rewrite_leaf_calls;
        drop(_on);

        assert!(rewrite_calls >= 32, "rewrite path rebuilt {rewrite_calls}");
        assert_eq!(direct_calls, 0, "direct path rebuilt the leaf");
        for i in 0..32_u64 {
            let key = format!("k{i:04}");
            assert_eq!(
                rewrite
                    .point_lookup(key.as_bytes())
                    .expect("rewrite lookup"),
                direct.point_lookup(key.as_bytes()).expect("direct lookup")
            );
        }
        assert!(direct.validate().expect("validate").errors.is_empty());
    }

    #[test]
    fn direct_unique_conflict_matches_rewrite() {
        let _off = ForceGuard::set(false);
        let (_rewrite_dir, rewrite) = fresh(IndexUniqueness::Unique).expect("rewrite unique");
        rewrite.insert_unique(1, b"same", row(1, 0)).expect("first");
        let rewrite_err = rewrite
            .insert_unique(1, b"same", row(2, 0))
            .expect_err("conflict");
        drop(_off);

        let _on = ForceGuard::set(true);
        let (_direct_dir, direct) = fresh(IndexUniqueness::Unique).expect("direct unique");
        direct.insert_unique(1, b"same", row(1, 0)).expect("first");
        let direct_err = direct
            .insert_unique(1, b"same", row(2, 0))
            .expect_err("conflict");

        assert!(matches!(rewrite_err, Error::WriteConflict));
        assert!(matches!(direct_err, Error::WriteConflict));
        assert_eq!(
            rewrite.point_lookup(b"same").expect("rewrite"),
            direct.point_lookup(b"same").expect("direct")
        );
    }
}
