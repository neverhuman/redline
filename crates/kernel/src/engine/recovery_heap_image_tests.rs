//! `redoes_heap_page_image` says when heap redo will replace a heap page
//! whole, so recovery loads the row directory after redo instead of before.

use std::collections::HashMap;

use super::timeline::ReplayFilter;
use super::{RecoveryTarget, redoes_heap_page_image};
use crate::format::{Csn, Lsn, Page, PageId, PageKind, RelId, TxId};
use crate::wal::{WalPayload, WalRecord, WalRecordKind};

const COMMITTED: TxId = TxId(7);
const UNCOMMITTED: TxId = TxId(8);

fn image(lsn: u64, tx_id: TxId, kind: PageKind) -> WalRecord {
    let page = Page::new(4096, kind, PageId(3), RelId(1)).unwrap();
    WalRecord {
        lsn: Lsn(lsn),
        prev_lsn: Lsn::ZERO,
        tx_id,
        kind: WalRecordKind::PageImage,
        payload: WalPayload::PageImage {
            page_id: PageId(3),
            page_lsn: Lsn::ZERO,
            page_bytes: page.as_bytes().to_vec(),
        }
        .encode()
        .unwrap(),
    }
}

fn redoes(records: &[WalRecord], replay_from: u64) -> bool {
    let filter = ReplayFilter::new(records, RecoveryTarget::Latest).unwrap();
    let committed = HashMap::from([(COMMITTED, Csn(1))]);
    redoes_heap_page_image(records, Lsn(replay_from), &filter, &committed).unwrap()
}

#[test]
fn a_committed_heap_image_from_the_redo_point_is_redone() {
    assert!(redoes(&[image(100, COMMITTED, PageKind::Heap)], 100));
}

#[test]
fn other_images_leave_the_directory_load_before_redo() {
    // An index page, an uncommitted transaction's page, and an image the
    // checkpointed pages already hold.
    assert!(!redoes(&[image(100, COMMITTED, PageKind::BtreeLeaf)], 50));
    assert!(!redoes(&[image(100, UNCOMMITTED, PageKind::Heap)], 50));
    assert!(!redoes(&[image(100, COMMITTED, PageKind::Heap)], 200));
}
