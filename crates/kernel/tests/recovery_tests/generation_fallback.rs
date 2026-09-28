//! When recovery may and may not fall back to the older control
//! generation (workplan R6).
//!
//! A slot that does not decode may have held the newest generation, whose
//! pages are already in the page file. The valid slot beside it is only a
//! fallback, usable when the WAL still holds everything after it; without
//! that WAL nothing says which rows the newer checkpoint wrote, and the
//! open must fail with "previous checkpoint generation lacks required WAL". A newer
//! generation whose transaction status belongs to another generation is
//! unusable, and recovery falls back past it.

use redlinedb_kernel::Error;
use redlinedb_kernel::engine::Engine;

use super::generations::{
    assert_rows_exact, assert_scan_finds_each_row_once, corrupt_byte, files_under, open_err,
    two_generations_over_pruned_segments,
};
use super::wal_segment_count;

const PREVIOUS_LACKS_WAL: Error =
    Error::CorruptWal("previous checkpoint generation lacks required WAL");

#[test]
fn corrupt_newer_control_with_the_fallback_wal_missing_fails_closed() {
    let image = two_generations_over_pruned_segments();
    let root = image.temp.path();
    corrupt_byte(&root.join("CONTROL_B"), 24);
    // Remove the segment generation 1 starts replaying from.
    let segment_bytes = image.config.wal.segment_bytes;
    let first_segment = image.first.checkpoint_lsn.0 / segment_bytes + 1;
    let wal = root.join("wal");
    assert!(
        wal_segment_count(&wal).contains(&first_segment),
        "generation 1's first segment {first_segment} should still be on disk"
    );
    std::fs::remove_file(wal.join(format!("{first_segment:020}.wal"))).unwrap();
    let before = files_under(root);

    assert_eq!(open_err(root, image.config.clone()), PREVIOUS_LACKS_WAL);
    assert_eq!(files_under(root), before, "a failed open changed files");
}

#[test]
fn corrupt_newer_control_with_no_wal_at_all_fails_closed() {
    let image = two_generations_over_pruned_segments();
    let root = image.temp.path();
    corrupt_byte(&root.join("CONTROL_B"), 24);
    // An empty WAL passes generation 1's own checks, but cannot say what
    // the unreadable newer generation wrote to the page file.
    std::fs::remove_dir_all(root.join("wal")).unwrap();
    let before = files_under(root);

    assert_eq!(open_err(root, image.config.clone()), PREVIOUS_LACKS_WAL);
    assert_eq!(files_under(root), before, "a failed open changed files");
}

#[test]
fn a_newer_generation_with_another_generations_tx_status_falls_back() {
    let image = two_generations_over_pruned_segments();
    let root = image.temp.path();
    let status = |generation: u64| root.join(format!("TX_STATUS_{generation:020}"));
    std::fs::copy(
        status(image.first.generation),
        status(image.second.generation),
    )
    .unwrap();

    let (reopened, report) = Engine::open_with_recovery_report(root, image.config.clone()).unwrap();
    assert_eq!(reopened.checkpoint_info().unwrap(), Some(image.first));
    assert_eq!(report.skipped_generation, Some(image.second.generation));
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("generation mismatch")),
        "{:?}",
        report.warnings
    );
    assert_rows_exact(&reopened, &image.rows);
}

/// The open duplicate-row defect (docs/launch/v5.0.0-evidence.md, "Known
/// defects at release"): after a fallback
/// through a corrupt newer control slot, heap redo from the older
/// generation adds a second copy of each row the newer checkpoint wrote.
#[test]
#[ignore = "open defect: heap redo after a fallback past a corrupt newer control slot duplicates rows that checkpoint wrote"]
fn fallback_after_corrupt_newer_control_scans_each_row_once() {
    let image = two_generations_over_pruned_segments();
    let root = image.temp.path();
    corrupt_byte(&root.join("CONTROL_B"), 24);
    let reopened = Engine::open(root, image.config.clone()).unwrap();
    assert_scan_finds_each_row_once(&reopened, &image.rows);
}
