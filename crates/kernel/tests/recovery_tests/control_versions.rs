//! Control files from other builds.
//!
//! A version-1 control file comes from v4.1.0, whose checkpoint did not
//! write every dirty page: a heap page past its `page_count` can hold rows
//! whose WAL that checkpoint already covered. Recovery must not empty such
//! a page. A control file with a version this build does not know was
//! written by a newer build; recovering past it would replay heap records
//! that build's page file may already hold, so the open must refuse.

use std::path::Path;

use redlinedb_kernel::Error;
use redlinedb_kernel::engine::Engine;
use redlinedb_kernel::format::RowId;
use redlinedb_kernel::format::bytes::crc32_with_zeroed_field;
use redlinedb_kernel::storage::ControlFile;
use redlinedb_kernel::txn::Isolation;
use tempfile::TempDir;

use super::config;
use super::generations::{assert_rows_read_back, files_under, open_err, payload};

const CHECKSUM_OFFSET: usize = 8;

/// Rewrite the control file at `path` with `version` and a valid checksum,
/// after `edit` changed its fields.
fn rewrite_control(path: &Path, version: u32, edit: impl FnOnce(&mut ControlFile)) {
    let mut control = ControlFile::decode(&std::fs::read(path).unwrap()).unwrap();
    edit(&mut control);
    let mut bytes = control.encode().unwrap();
    bytes[4..8].copy_from_slice(&version.to_le_bytes());
    if version == 1 {
        // Version 1 has no heap redo LSN; those bytes were zero.
        bytes[40..48].fill(0);
    }
    let checksum = crc32_with_zeroed_field(&bytes, CHECKSUM_OFFSET);
    bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&checksum.to_le_bytes());
    std::fs::write(path, bytes).unwrap();
}

/// A closed database with one checkpoint over `count` rows spread across
/// several heap pages.
fn checkpointed_rows(count: u64) -> (TempDir, Vec<(RowId, Vec<u8>)>, ControlFile) {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let mut rows = Vec::new();
    for tag in 0..count {
        let bytes = payload(tag);
        let mut tx = engine.begin(Isolation::Snapshot).unwrap();
        let row = engine.insert(&mut tx, bytes.clone()).unwrap();
        engine.commit(tx).unwrap();
        rows.push((row, bytes));
    }
    let checkpoint = engine.checkpoint().unwrap();
    drop(engine);
    assert!(
        checkpoint.page_count > 2,
        "the rows should span several pages, got {}",
        checkpoint.page_count
    );
    (temp, rows, checkpoint)
}

#[test]
fn a_version_one_checkpoint_keeps_heap_pages_past_its_page_count() {
    let (temp, rows, checkpoint) = checkpointed_rows(40);
    // v4.1.0's checkpoint could record a page count below heap pages that
    // eviction wrote, holding rows below the checkpoint LSN. Model that by
    // recording page 1 as the last page the checkpoint covered.
    rewrite_control(
        &temp.path().join(format!(
            "CONTROL_{}",
            if checkpoint.generation.is_multiple_of(2) {
                "B"
            } else {
                "A"
            }
        )),
        1,
        |control| {
            control.page_count = 1;
            control.heap_redo_lsn = control.checkpoint_lsn;
        },
    );

    let reopened = Engine::open(temp.path(), config()).unwrap();
    assert_rows_read_back(&reopened, &rows);
    drop(reopened);
    let reopened = Engine::open(temp.path(), config()).unwrap();
    assert_rows_read_back(&reopened, &rows);
}

#[test]
fn a_control_file_from_a_newer_build_fails_the_open() {
    let (temp, _rows, checkpoint) = checkpointed_rows(20);
    let slot = if checkpoint.generation.is_multiple_of(2) {
        "CONTROL_B"
    } else {
        "CONTROL_A"
    };
    rewrite_control(&temp.path().join(slot), 3, |_| {});
    let before = files_under(temp.path());
    assert_eq!(
        open_err(temp.path(), config()),
        Error::UnsupportedVersion(3)
    );
    assert_eq!(
        files_under(temp.path()),
        before,
        "a failed open changed files"
    );
}

#[test]
fn a_newer_build_slot_beside_a_known_one_fails_the_open() {
    let (temp, mut rows, first) = checkpointed_rows(20);
    let engine = Engine::open(temp.path(), config()).unwrap();
    let bytes = payload(rows.len() as u64);
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    let row = engine.insert(&mut tx, bytes.clone()).unwrap();
    engine.commit(tx).unwrap();
    rows.push((row, bytes));
    let second = engine.checkpoint().unwrap();
    drop(engine);
    assert!(second.generation > first.generation);
    let newest = if second.generation.is_multiple_of(2) {
        "CONTROL_B"
    } else {
        "CONTROL_A"
    };
    // The newest slot now says a newer build wrote it; the other slot is
    // still a valid version-2 file. Falling back to it would recover over
    // a page file the newer build wrote.
    rewrite_control(&temp.path().join(newest), 3, |_| {});
    let before = files_under(temp.path());
    assert_eq!(
        open_err(temp.path(), config()),
        Error::UnsupportedVersion(3)
    );
    assert_eq!(
        files_under(temp.path()),
        before,
        "a failed open changed files"
    );
}
