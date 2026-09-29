use crate::format::{Lsn, Page, PageGeneration, PageId, PageKind, SLOT_LEN, TxId};
use crate::storage::PageGuard;
use crate::{Error, Result};

use super::super::cells::Entry;
use super::super::{BtreeIndex, INDEX_SPECIAL_LEN, IndexRowRef, PAGE_INTERNAL_KIND, PageHeader};

/// One page a split changes: its pinned frame and its new image.
struct StagedPage {
    guard: PageGuard,
    page: Page,
}

impl StagedPage {
    fn id(&self) -> PageId {
        self.guard.page_id()
    }
}

/// One level of a split, bottom-up: the page that splits and its new right
/// sibling, or at the top the parent that takes the last separator.
struct StagedLevel {
    left: StagedPage,
    right: Option<StagedPage>,
}

/// A new root over the old one, and the meta page that names it.
struct StagedRoot {
    root: StagedPage,
    meta: StagedPage,
}

impl BtreeIndex {
    /// Split `leaf_id`, insert the entry, and add the separators and pages
    /// the split needs up the tree, all or nothing.
    ///
    /// Every page the split changes is pinned, read, allocated and staged in
    /// memory before the first WAL record or install. An allocation or pin
    /// fails when the pool has no frame to give; failing there leaves the
    /// tree as it was. The split used to install the two leaf halves first
    /// and allocate the parent's pages afterwards, so a failure left a leaf
    /// with a right sibling its parent never named, or a root leaf with a
    /// sibling and no root above it. The next split under it then took that
    /// leaf for its parent and failed with "expected internal page".
    #[allow(clippy::too_many_arguments)]
    pub(in crate::index) fn split_leaf_and_insert(
        &self,
        ancestors: &[PageId],
        leaf_id: PageId,
        logical_key: &[u8],
        row: IndexRowRef,
        physical: Vec<u8>,
        tx_id: crate::format::TxId,
        emit_wal: bool,
        lsn: crate::format::Lsn,
    ) -> Result<()> {
        // Lane E failpoint: armed at the start of leaf split, before any
        // structural change is applied. Crashing here exercises recovery from
        // a half-applied split (no new pages allocated yet on disk).
        crate::fail_point!("index::split");
        self.record_leaf_split();
        let rel_id = self.descriptor().rel_id;
        let index_id = self.descriptor().index_id;
        let leaf_latch = self.inner.latches.get(leaf_id);
        let leaf_write = leaf_latch.write();
        let guard = self.inner.buffer.pin(leaf_id)?;
        let (left_entries, right_entries, left_high, header) = guard.with_page(|page| {
            let mut entries = self.read_entries(page)?;
            entries.push(Entry::Leaf {
                logical_key: logical_key.to_vec(),
                row,
                physical,
                create_tx: tx_id,
                delete_tx: TxId::ZERO,
            });
            entries.sort_by(|a, b| a.compare(b));
            // Pick a split position that does not cleave a duplicate-key run
            // unless every entry on the page shares one logical key. This keeps
            // `point_lookup` correct under the existing right-walk traversal:
            // every duplicate of a given logical_key sits on a single leaf,
            // and the parent separator is the right-half's first key (strictly
            // greater than every key on the left half).
            let split = Self::choose_leaf_split(&entries);
            let right_entries = entries.split_off(split);
            // Most splits land on a clean key boundary (chosen by
            // `choose_leaf_split`) and can use the right half's first logical
            // key as a compact separator. When duplicates of one logical key
            // span both halves (only possible when the page held one logical
            // key throughout), fall back to the right half's first *physical*
            // key (logical_key || row_ref suffix) so the separator is strictly
            // greater than every left-side entry.
            let left_high = match (entries.last(), right_entries.first()) {
                (Some(last_left), Some(first_right))
                    if last_left.logical_key() == first_right.logical_key() =>
                {
                    match first_right.physical().map(|p| p.to_vec()) {
                        Some(vec) => vec,
                        None => Vec::new(),
                    }
                }
                _ => match right_entries.first().and_then(|entry| entry.logical_key()) {
                    Some(key) => key.to_vec(),
                    None => Vec::new(),
                },
            };
            let header = Self::read_page_header(page)?;
            Ok((entries, right_entries, left_high, header))
        })?;
        let right_guard = self.inner.buffer.allocate(PageKind::BtreeLeaf, rel_id)?;
        let right_latch = self.inner.latches.get(right_guard.page_id());
        let right_write = right_latch.write();
        let mut left_page = guard.with_page(|page| Ok(page.clone()))?;
        Self::rewrite_leaf(
            &mut left_page,
            index_id,
            &left_entries,
            header.left,
            Some(right_guard.page_id()),
            left_high.clone(),
        )?;
        let mut right_page = right_guard.with_page(|page| Ok(page.clone()))?;
        right_page.reinitialize_with_special(
            PageKind::BtreeLeaf,
            right_guard.page_id(),
            rel_id,
            PageGeneration::ONE,
            INDEX_SPECIAL_LEN,
        )?;
        Self::rewrite_leaf(
            &mut right_page,
            index_id,
            &right_entries,
            Some(leaf_id),
            header.right,
            header.high_key.clone(),
        )?;
        let mut levels = vec![StagedLevel {
            left: StagedPage {
                guard,
                page: left_page,
            },
            right: Some(StagedPage {
                guard: right_guard,
                page: right_page,
            }),
        }];
        let root = self.stage_parents(ancestors, &mut levels, left_high)?;

        // Each page image below can be durable before its page is installed.
        // Hold the checkpoint at the first image until both leaves, every
        // parent or new root, and the meta page are installed: recovery
        // skips records older than the checkpoint.
        let install_fence = match &self.inner.wal {
            Some(wal) if emit_wal => Some(wal.begin_page_install()?),
            _ => None,
        };
        self.install_split(levels, root, tx_id, emit_wal, lsn)?;
        drop(install_fence);
        drop(right_write);
        drop(leaf_write);
        Ok(())
    }

    /// Stage the separator `separator` for the right half of `levels[0]` in
    /// each parent up the tree, splitting parents that overflow, and a new
    /// root when the old root splits. Pushes one level per parent it changes.
    /// Only splits change internal pages, and they hold the structure lock,
    /// so the parents read here are the ones the install replaces.
    fn stage_parents(
        &self,
        ancestors: &[PageId],
        levels: &mut Vec<StagedLevel>,
        separator: Vec<u8>,
    ) -> Result<Option<StagedRoot>> {
        let meta = self.meta()?;
        let rel_id = self.descriptor().rel_id;
        let mut ancestors = ancestors.to_vec();
        let first = levels
            .first()
            .ok_or_else(|| Error::CorruptPage("split has no leaf level"))?;
        let mut current_left = first.left.id();
        let mut current_right = first
            .right
            .as_ref()
            .ok_or_else(|| Error::CorruptPage("split has no right leaf"))?
            .id();
        let mut current_separator = separator;
        let mut left_level = 1_u16;

        while let Some(parent_id) = ancestors.pop() {
            let guard = self.inner.buffer.pin(parent_id)?;
            let (header, mut entries, body_capacity) = guard.with_page(|page| {
                let header = Self::read_page_header(page)?;
                let entries = self.read_entries(page)?;
                let capacity = page
                    .as_bytes()
                    .len()
                    .saturating_sub(crate::format::PAGE_HEADER_LEN + INDEX_SPECIAL_LEN);
                Ok((header, entries, capacity))
            })?;
            if header.kind != PAGE_INTERNAL_KIND {
                return Err(Error::CorruptPage("expected internal page"));
            }
            let parent_level = header.level;
            entries.push(Entry::Internal {
                separator: current_separator.clone(),
                child: current_right,
            });
            entries.sort_by(|a, b| a.compare(b));
            let required = Self::encoded_entries_len(&entries) + entries.len() * SLOT_LEN;
            if required <= body_capacity {
                let mut staged = guard.with_page(|page| Ok(page.clone()))?;
                Self::rewrite_internal(
                    &mut staged,
                    meta.index_id,
                    header.level,
                    &entries,
                    header.left,
                    header.right,
                    header.high_key.clone(),
                )?;
                levels.push(StagedLevel {
                    left: StagedPage {
                        guard,
                        page: staged,
                    },
                    right: None,
                });
                return Ok(None);
            }

            let split = entries.len() / 2;
            let right_entries = entries.split_off(split);
            let left_entries = entries;
            let right_left_child = match right_entries.first() {
                Some(Entry::Internal { child, .. }) => *child,
                _ => {
                    return Err(Error::CorruptPage(
                        "internal split produced empty right side",
                    ));
                }
            };
            let right_separator = {
                let staged: Vec<(PageId, &Page)> = levels
                    .iter()
                    .flat_map(|level| std::iter::once(&level.left).chain(level.right.as_ref()))
                    .map(|staged| (staged.id(), &staged.page))
                    .collect();
                self.min_key_for_staged_page(right_left_child, &staged)?
            };
            let right_guard = self
                .inner
                .buffer
                .allocate(PageKind::BtreeInternal, rel_id)?;
            let mut left_page = guard.with_page(|page| Ok(page.clone()))?;
            Self::rewrite_internal(
                &mut left_page,
                meta.index_id,
                parent_level,
                &left_entries,
                header.left,
                Some(right_guard.page_id()),
                right_separator.clone(),
            )?;
            let mut right_page = right_guard.with_page(|page| Ok(page.clone()))?;
            right_page.reinitialize_with_special(
                PageKind::BtreeInternal,
                right_guard.page_id(),
                rel_id,
                PageGeneration::ONE,
                INDEX_SPECIAL_LEN,
            )?;
            Self::rewrite_internal(
                &mut right_page,
                meta.index_id,
                parent_level,
                &right_entries,
                Some(right_left_child),
                header.right,
                header.high_key.clone(),
            )?;
            current_left = parent_id;
            current_right = right_guard.page_id();
            current_separator = right_separator;
            left_level = parent_level.saturating_add(1);
            levels.push(StagedLevel {
                left: StagedPage {
                    guard,
                    page: left_page,
                },
                right: Some(StagedPage {
                    guard: right_guard,
                    page: right_page,
                }),
            });
        }

        let root_guard = self
            .inner
            .buffer
            .allocate(PageKind::BtreeInternal, rel_id)?;
        let mut root_page = root_guard.with_page(|page| Ok(page.clone()))?;
        root_page.reinitialize_with_special(
            PageKind::BtreeInternal,
            root_guard.page_id(),
            rel_id,
            PageGeneration::ONE,
            INDEX_SPECIAL_LEN,
        )?;
        Self::write_page_header(
            &mut root_page,
            &PageHeader {
                kind: PAGE_INTERNAL_KIND,
                level: left_level,
                index_id: meta.index_id,
                left: Some(current_left),
                right: None,
                high_key: Vec::new(),
            },
        )?;
        let entries = vec![Entry::Internal {
            separator: current_separator,
            child: current_right,
        }];
        Self::rewrite_internal(
            &mut root_page,
            meta.index_id,
            left_level,
            &entries,
            Some(current_left),
            None,
            Vec::new(),
        )?;
        let meta_guard = self.inner.buffer.pin(self.inner.meta_page_id)?;
        let mut meta_page = meta_guard.with_page(|page| Ok(page.clone()))?;
        let mut root_meta = Self::read_meta(&meta_page)?;
        root_meta.root_page_id = root_guard.page_id();
        root_meta.root_level = left_level;
        Self::write_meta(&mut meta_page, &root_meta)?;
        Ok(Some(StagedRoot {
            root: StagedPage {
                guard: root_guard,
                page: root_page,
            },
            meta: StagedPage {
                guard: meta_guard,
                page: meta_page,
            },
        }))
    }

    /// Log and install a staged split, bottom-up. Each level's new right page
    /// is logged before the page that links to it, so a replayed link never
    /// names a page whose image the WAL lacks. The caller holds the leaf
    /// level's latches; each higher level is latched while it is installed.
    fn install_split(
        &self,
        levels: Vec<StagedLevel>,
        root: Option<StagedRoot>,
        tx_id: crate::format::TxId,
        emit_wal: bool,
        lsn: Lsn,
    ) -> Result<()> {
        let log = |page: &Page| {
            if emit_wal {
                self.record_staged_page_image(page, tx_id)
            } else {
                Ok(lsn)
            }
        };
        for (depth, level) in levels.into_iter().enumerate() {
            let latches: Vec<_> = if depth == 0 {
                Vec::new()
            } else {
                std::iter::once(&level.left)
                    .chain(level.right.as_ref())
                    .map(|staged| self.inner.latches.get(staged.id()))
                    .collect()
            };
            let _writes: Vec<_> = latches.iter().map(|latch| latch.write()).collect();
            let right_lsn = match &level.right {
                Some(right) => Some(log(&right.page)?),
                None => None,
            };
            let left_lsn = log(&level.left.page)?;
            level.left.guard.install_dirty(level.left.page, left_lsn)?;
            if let (Some(right), Some(right_lsn)) = (level.right, right_lsn) {
                right.guard.install_dirty(right.page, right_lsn)?;
            }
        }
        if let Some(root) = root {
            let root_latch = self.inner.latches.get(root.root.id());
            let _root_write = root_latch.write();
            let root_lsn = log(&root.root.page)?;
            root.root.guard.install_dirty(root.root.page, root_lsn)?;
            let meta_lsn = log(&root.meta.page)?;
            root.meta.guard.install_dirty(root.meta.page, meta_lsn)?;
        }
        Ok(())
    }
}
