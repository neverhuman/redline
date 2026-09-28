use crate::Error;
use crate::format::{Lsn, TxId};
use crate::storage::ControlFile;
use crate::wal::{WalRecord, WalRecordKind, WalRecordLink, WalScanReport, WalSegmentInfo};

use super::{RecoveryPlan, validate_recovery_plan};

const SEGMENT: u64 = 4096;

/// A scan whose first record sits at `first` with previous LSN `prev`, whose
/// log ends at `end`, over the given segment numbers.
fn scan(first: Option<(u64, u64)>, end: u64, segments: &[u64]) -> WalScanReport {
    let records = first
        .map(|(lsn, prev)| {
            vec![WalRecord {
                lsn: Lsn(lsn),
                prev_lsn: Lsn(prev),
                tx_id: TxId(1),
                kind: WalRecordKind::Commit,
                payload: Vec::new(),
            }]
        })
        .unwrap_or_default();
    WalScanReport {
        first_record: first.map(|(lsn, prev)| WalRecordLink {
            lsn: Lsn(lsn),
            prev_lsn: Lsn(prev),
        }),
        records,
        valid_end_lsn: Lsn(end),
        torn_tail: false,
        segments: segments
            .iter()
            .map(|number| WalSegmentInfo {
                number: *number,
                len: 1,
            })
            .collect(),
        tail: None,
    }
}

fn checkpoint(checkpoint_lsn: u64, heap_redo_lsn: u64) -> ControlFile {
    ControlFile {
        generation: 1,
        checkpoint_lsn: Lsn(checkpoint_lsn),
        page_count: 1,
        heap_redo_lsn: Lsn(heap_redo_lsn),
    }
}

/// The plan with no checkpoint: replay everything, over no heap pages.
fn from_start() -> RecoveryPlan {
    RecoveryPlan {
        replay_from_lsn: Lsn::ZERO,
        heap_replay_from: Lsn::ZERO,
        heap_page_count: 0,
    }
}

/// The plan for a checkpoint from [`checkpoint`], whose page count is 1.
fn plan(from: u64, heap: u64) -> RecoveryPlan {
    RecoveryPlan {
        replay_from_lsn: Lsn(from),
        heap_replay_from: Lsn(heap),
        heap_page_count: 1,
    }
}

#[test]
fn a_wal_from_lsn_zero_needs_no_checkpoint() {
    let scan = scan(Some((0, 0)), 9000, &[1, 2, 3]);
    assert_eq!(
        validate_recovery_plan(&scan, None, SEGMENT),
        Ok(from_start())
    );
}

#[test]
fn without_a_checkpoint_the_wal_must_start_at_zero() {
    let expected = Err(Error::CorruptWal(
        "wal does not start at lsn 0 and no checkpoint covers the records before it",
    ));
    let scan_from_two = scan(Some((SEGMENT, 4000)), 9000, &[2, 3]);
    assert_eq!(
        validate_recovery_plan(&scan_from_two, None, SEGMENT),
        expected
    );
    // A log restarted at a segment boundary names no predecessor, which is
    // still not the start of the log.
    let restarted = scan(Some((SEGMENT, 0)), 5000, &[2]);
    assert_eq!(validate_recovery_plan(&restarted, None, SEGMENT), expected);
    // No record at all, yet a segment past the first: those records are gone.
    let emptied = scan(None, 0, &[1, 2]);
    assert_eq!(validate_recovery_plan(&emptied, None, SEGMENT), expected);
}

#[test]
fn an_empty_first_segment_needs_no_checkpoint() {
    assert_eq!(
        validate_recovery_plan(&scan(None, 0, &[1]), None, SEGMENT),
        Ok(from_start())
    );
    assert_eq!(
        validate_recovery_plan(&scan(None, 0, &[]), None, SEGMENT),
        Ok(from_start())
    );
}

#[test]
fn a_gap_after_the_first_record_segment_fails() {
    let scan = scan(Some((0, 0)), 100, &[1, 3]);
    assert_eq!(
        validate_recovery_plan(&scan, None, SEGMENT),
        Err(Error::CorruptWal(
            "wal segment missing between retained segments"
        ))
    );
}

#[test]
fn empty_segments_below_the_first_record_are_ignored() {
    // A log the append floor restarted in segment 4 above an emptied
    // segment 2.
    let scan = scan(Some((3 * SEGMENT, 0)), 3 * SEGMENT + 90, &[2, 4]);
    let checkpoint = checkpoint(500, 600);
    assert_eq!(
        validate_recovery_plan(&scan, Some(&checkpoint), SEGMENT),
        Ok(plan(500, 600))
    );
}

#[test]
fn a_checkpoint_needs_the_wal_from_its_redo_start() {
    let checkpoint = checkpoint(2 * SEGMENT + 10, 2 * SEGMENT + 20);
    // The first record is at or below the checkpoint.
    let covered = scan(Some((2 * SEGMENT, SEGMENT + 7)), 3 * SEGMENT, &[3, 4]);
    assert_eq!(
        validate_recovery_plan(&covered, Some(&checkpoint), SEGMENT),
        Ok(plan(2 * SEGMENT + 10, 2 * SEGMENT + 20))
    );
    // Its predecessor was below the checkpoint, so nothing from the
    // checkpoint on is missing.
    let after = scan(Some((3 * SEGMENT, 2 * SEGMENT + 5)), 4 * SEGMENT, &[4]);
    assert!(validate_recovery_plan(&after, Some(&checkpoint), SEGMENT).is_ok());
    // Its predecessor was past the checkpoint: records are missing.
    let missing = scan(Some((3 * SEGMENT, 2 * SEGMENT + 100)), 4 * SEGMENT, &[4]);
    assert_eq!(
        validate_recovery_plan(&missing, Some(&checkpoint), SEGMENT),
        Err(Error::CorruptWal(
            "wal starts after the checkpoint redo lsn"
        ))
    );
}

#[test]
fn a_checkpoint_needs_the_wal_through_its_heap_redo_lsn() {
    let checkpoint = checkpoint(100, 900);
    let short = scan(Some((0, 0)), 800, &[1]);
    assert_eq!(
        validate_recovery_plan(&short, Some(&checkpoint), SEGMENT),
        Err(Error::CorruptWal("wal ends before checkpoint redo lsn"))
    );
}

#[test]
fn an_empty_wal_after_a_checkpoint_is_left_to_the_append_floor() {
    let checkpoint = checkpoint(SEGMENT + 10, SEGMENT + 10);
    // Nothing below the checkpoint remains: the floor restarts the log.
    assert!(validate_recovery_plan(&scan(None, 0, &[]), Some(&checkpoint), SEGMENT).is_ok());
    assert!(validate_recovery_plan(&scan(None, 0, &[3]), Some(&checkpoint), SEGMENT).is_ok());
    // A segment below the checkpoint remains but lost its records.
    assert_eq!(
        validate_recovery_plan(&scan(None, 0, &[2]), Some(&checkpoint), SEGMENT),
        Err(Error::CorruptWal("wal ends before checkpoint redo lsn"))
    );
}
