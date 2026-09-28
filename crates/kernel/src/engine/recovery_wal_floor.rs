//! Where a reopened WAL appends its next record (workplan R3).
//!
//! Recovery replays records from the checkpoint's redo LSN. A record below
//! that LSN is never replayed, although its commit is still registered, so
//! a WAL that restarts below the checkpoint loses every change made after
//! the reopen at the next restart. Page image redo also skips an image whose
//! LSN is not past the page's own, so the WAL must restart past every page
//! LSN as well.

use crate::format::Lsn;
use crate::wal::{WalOpenScanSummary, WalScanReport};
use crate::{Error, Result};

/// Where the WAL resumes, and a segment to empty before it does.
#[derive(Debug)]
pub(super) struct WalResume {
    pub(super) summary: WalOpenScanSummary,
    /// Set when no record survives, the highest segment on disk still holds
    /// bytes, and the log resumes past that segment. The scan read those
    /// bytes as a torn tail, which recovery discards anyway. Left in a
    /// segment that is no longer the last, they would fail the next scan.
    pub(super) torn_segment_to_empty: Option<u64>,
}

/// Choose where the WAL resumes after `scan`.
///
/// `segments` lists the segment numbers on disk and whether each holds any
/// bytes. `max_page_lsn` reads the highest LSN in the page file; it runs only
/// when the WAL holds no records, which is rare, because it reads every page.
///
/// - The WAL reaches the checkpoint: resume at its end, as before.
/// - It ends below the checkpoint while a segment that should hold the
///   records up to the checkpoint is still there: those records were lost,
///   not pruned. Fail closed rather than restart below the checkpoint.
/// - No segment below the checkpoint remains and no record survives, as
///   after the WAL directory was removed or a checkpoint that ended exactly
///   on a segment boundary pruned the segment before it: resume at the next
///   segment boundary at or past the checkpoint, the highest page LSN and
///   the start of the highest segment on disk.
pub(super) fn wal_resume_position(
    scan: &WalScanReport,
    checkpoint_lsn: Lsn,
    segments: &[(u64, bool)],
    segment_bytes: u64,
    max_page_lsn: impl FnOnce() -> Result<Lsn>,
) -> Result<WalResume> {
    let as_is = |scan: &WalScanReport| WalResume {
        summary: scan.open_summary(),
        torn_segment_to_empty: None,
    };
    if !scan.records.is_empty() && scan.valid_end_lsn >= checkpoint_lsn {
        return Ok(as_is(scan));
    }
    let segment_start = |segment: u64| Lsn(segment.saturating_sub(1).saturating_mul(segment_bytes));
    if scan.valid_end_lsn < checkpoint_lsn
        && segments
            .iter()
            .any(|(segment, _)| segment_start(*segment) < checkpoint_lsn)
    {
        return Err(Error::CorruptWal("wal ends before checkpoint redo lsn"));
    }
    // No record survives. The scan accepts bytes without a whole record
    // only as the tail of the last segment, so every other segment is empty,
    // and none starts above the floor.
    let highest = segments.iter().copied().max_by_key(|(segment, _)| *segment);
    let highest_start = highest.map_or(Lsn::ZERO, |(segment, _)| segment_start(segment));
    let floor = checkpoint_lsn.max(highest_start).max(max_page_lsn()?);
    if floor == Lsn::ZERO {
        return Ok(as_is(scan));
    }
    let summary = WalOpenScanSummary::at_floor(floor, segment_bytes)?;
    let torn_segment_to_empty = highest
        .filter(|(segment, has_bytes)| {
            *has_bytes && segment_start(*segment) < summary.valid_end_lsn
        })
        .map(|(segment, _)| segment);
    Ok(WalResume {
        summary,
        torn_segment_to_empty,
    })
}

#[cfg(test)]
mod tests {
    use super::{WalResume, wal_resume_position};
    use crate::format::{Lsn, TxId};
    use crate::wal::{WalRecord, WalRecordKind, WalScanReport};
    use crate::{Error, Result};

    const SEGMENT: u64 = 4096;

    fn scan(end: Option<u64>) -> WalScanReport {
        let records = end
            .map(|end| {
                vec![WalRecord {
                    lsn: Lsn(end - 48),
                    prev_lsn: Lsn::ZERO,
                    tx_id: TxId(1),
                    kind: WalRecordKind::Commit,
                    payload: Vec::new(),
                }]
            })
            .unwrap_or_default();
        WalScanReport {
            valid_end_lsn: Lsn(end.unwrap_or(0)),
            records,
            torn_tail: false,
            segments: Vec::new(),
            first_record: None,
            tail: None,
        }
    }

    fn no_pages() -> Result<Lsn> {
        Ok(Lsn::ZERO)
    }

    fn resume(resume: WalResume) -> (Lsn, Lsn) {
        assert_eq!(resume.torn_segment_to_empty, None);
        (resume.summary.valid_end_lsn, resume.summary.last_record_lsn)
    }

    #[test]
    fn a_wal_that_reaches_the_checkpoint_resumes_at_its_end() {
        let summary =
            wal_resume_position(&scan(Some(500)), Lsn(500), &[(1, true)], SEGMENT, || {
                panic!("the page file is read only when no record survives")
            })
            .unwrap();
        assert_eq!(resume(summary), (Lsn(500), Lsn(452)));
    }

    #[test]
    fn a_wal_that_ends_below_the_checkpoint_fails_closed() {
        let err = wal_resume_position(&scan(Some(100)), Lsn(500), &[(1, true)], SEGMENT, no_pages)
            .unwrap_err();
        assert_eq!(
            err,
            Error::CorruptWal("wal ends before checkpoint redo lsn")
        );
    }

    #[test]
    fn an_empty_segment_below_the_checkpoint_fails_closed() {
        let err = wal_resume_position(&scan(None), Lsn(500), &[(1, true)], SEGMENT, no_pages)
            .unwrap_err();
        assert_eq!(
            err,
            Error::CorruptWal("wal ends before checkpoint redo lsn")
        );
    }

    #[test]
    fn a_missing_wal_resumes_at_the_segment_after_the_checkpoint() {
        let summary = wal_resume_position(&scan(None), Lsn(500), &[], SEGMENT, no_pages).unwrap();
        assert_eq!(resume(summary), (Lsn(SEGMENT), Lsn::ZERO));
    }

    #[test]
    fn a_missing_wal_resumes_past_the_newest_page() {
        let summary = wal_resume_position(&scan(None), Lsn(500), &[], SEGMENT, || {
            Ok(Lsn(3 * SEGMENT + 1))
        })
        .unwrap();
        assert_eq!(resume(summary), (Lsn(4 * SEGMENT), Lsn::ZERO));
    }

    #[test]
    fn a_checkpoint_on_a_segment_boundary_resumes_in_the_kept_empty_segment() {
        // The checkpoint ended exactly where segment 3 starts, so pruning
        // removed segments 1 and 2 and kept an empty segment 3.
        let checkpoint = Lsn(2 * SEGMENT);
        let summary =
            wal_resume_position(&scan(None), checkpoint, &[(3, false)], SEGMENT, no_pages).unwrap();
        assert_eq!(resume(summary), (checkpoint, Lsn::ZERO));
    }

    #[test]
    fn an_empty_wal_never_resumes_below_a_segment_on_disk() {
        let summary = wal_resume_position(
            &scan(None),
            Lsn::ZERO,
            &[(1, false), (6, false)],
            SEGMENT,
            no_pages,
        )
        .unwrap();
        assert_eq!(resume(summary), (Lsn(5 * SEGMENT), Lsn::ZERO));
    }

    #[test]
    fn a_new_database_still_starts_at_zero() {
        let summary =
            wal_resume_position(&scan(None), Lsn::ZERO, &[(1, true)], SEGMENT, no_pages).unwrap();
        assert_eq!(resume(summary), (Lsn::ZERO, Lsn::ZERO));
    }

    #[test]
    fn a_floor_with_no_segment_boundary_above_it_fails() {
        let err = wal_resume_position(&scan(None), Lsn(u64::MAX - 1), &[], SEGMENT, no_pages)
            .unwrap_err();
        assert_eq!(err, Error::CorruptWal("lsn overflow"));
    }

    #[test]
    fn a_torn_highest_segment_below_the_resume_point_is_emptied() {
        let resume = wal_resume_position(&scan(None), Lsn(500), &[(2, true)], SEGMENT, || {
            Ok(Lsn(3 * SEGMENT + 1))
        })
        .unwrap();
        assert_eq!(resume.summary.valid_end_lsn, Lsn(4 * SEGMENT));
        assert_eq!(resume.torn_segment_to_empty, Some(2));
    }

    #[test]
    fn a_torn_segment_the_wal_resumes_in_is_left_to_the_open() {
        // Opening at the start of that segment truncates it already.
        let resume =
            wal_resume_position(&scan(None), Lsn(SEGMENT), &[(2, true)], SEGMENT, no_pages)
                .unwrap();
        assert_eq!(resume.summary.valid_end_lsn, Lsn(SEGMENT));
        assert_eq!(resume.torn_segment_to_empty, None);
    }
}
