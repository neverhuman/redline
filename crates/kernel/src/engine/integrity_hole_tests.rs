//! `integrity_check` and pages the page file has never received.
//!
//! A page gets an id when it is allocated and reaches the file only when a
//! checkpoint or eviction writes it. A higher page can reach the file first,
//! which leaves the lower one zero-filled there until its own write: a page
//! allocated while a checkpoint was taking its snapshot, or one a crash cut
//! off before any write, whose rows recovery replays onto new pages. Such a
//! page holds nothing the database refers to.

use tempfile::TempDir;

use super::buffer_eviction_tests::config;
use super::{CommitDurability, Engine};
use crate::format::{Page, PageId, PageKind, RelId};

#[test]
fn integrity_check_passes_pages_the_file_never_received() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config(CommitDurability::Normal, 64)).unwrap();
    let before = engine.integrity_check().unwrap();
    assert_eq!(before, Vec::<String>::new());
    // A page written far past the end leaves every page below it unwritten.
    let far = Page::new(4096, PageKind::Heap, PageId(40), RelId(1)).unwrap();
    engine.buffer.write_page_direct(&far).unwrap();
    assert_eq!(engine.integrity_check().unwrap(), Vec::<String>::new());
}

#[test]
fn integrity_check_still_reports_a_damaged_page() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config(CommitDurability::Normal, 64)).unwrap();
    let mut page = Page::new(4096, PageKind::Heap, PageId(3), RelId(1)).unwrap();
    page.set_page_lsn(crate::format::Lsn(9)).unwrap();
    engine.buffer.write_page_direct(&page).unwrap();
    // Flip one byte past the header: the checksum no longer matches.
    let path = temp.path().join(&engine.config.data_file_name);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[2 * 4096 + 200] ^= 0xff;
    std::fs::write(&path, &bytes).unwrap();
    let errors = engine.integrity_check().unwrap();
    assert_eq!(errors, vec!["page 3: invalid checksum".to_string()]);
}
