//! Which scanned WAL records recovery applies, and how a recovery to a
//! target stays at that target (workplan R9, step 5).
//!
//! Recovery to an LSN or CSN target replays the records below the target
//! and leaves the rest of the log in place. Before this module the next
//! ordinary open replayed that rest, so a restored database moved past its
//! target as soon as it was reopened. Now a targeted recovery that leaves
//! records past its target appends a `TimelineFork` record at the end of
//! the log, naming the LSN the target cut at, and flushes it before the
//! open returns. Every later recovery treats the records from that LSN up
//! to the fork record as abandoned: it replays none of them and publishes
//! none of their commits. Their transaction ids, row ids and CSNs stay
//! spent, and their bytes stay in the WAL until a checkpoint prunes them.
//!
//! A CSN target is cut at the first commit record past it. The engine takes
//! a commit's CSN under the WAL lock, so CSN order is WAL order and every
//! commit after that record is past the target too; a log where that does
//! not hold fails the open before recovery writes anything.

use crate::format::{Csn, Lsn, TimelineId};
use crate::wal::{WalPayload, WalRecord, WalRecordKind};
use crate::{Error, Result};

use super::RecoveryTarget;

/// The records a recovery applies: those below its target's cut that no
/// earlier timeline fork abandoned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ReplayFilter {
    target: RecoveryTarget,
    /// Records at or past this LSN are past the target.
    cut: Option<Lsn>,
    /// `[from, to)` ranges earlier forks abandoned.
    abandoned: Vec<(Lsn, Lsn)>,
    /// Fork records already in the log.
    forks: u64,
}

impl ReplayFilter {
    /// Read the forks in `records` and find where `target` cuts the log.
    /// Only reads; a log it cannot make sense of fails here.
    pub(super) fn new(records: &[WalRecord], target: RecoveryTarget) -> Result<Self> {
        let mut abandoned = Vec::new();
        for record in records {
            if record.kind != WalRecordKind::TimelineFork {
                continue;
            }
            let WalPayload::TimelineFork { fork_lsn, .. } = WalPayload::decode(&record.payload)?
            else {
                return Err(Error::CorruptWal(
                    "timeline fork record has another payload",
                ));
            };
            if fork_lsn > record.lsn {
                return Err(Error::CorruptWal(
                    "timeline fork names an lsn after its own record",
                ));
            }
            abandoned.push((fork_lsn, record.lsn));
        }
        let forks = abandoned.len() as u64;
        let mut filter = Self {
            target,
            cut: None,
            abandoned,
            forks,
        };
        filter.cut = match target {
            RecoveryTarget::Latest => None,
            RecoveryTarget::Lsn(limit) => Some(limit),
            RecoveryTarget::Csn(limit) => filter.csn_cut(records, limit)?,
        };
        Ok(filter)
    }

    /// The LSN of the first commit record, outside the abandoned ranges,
    /// whose CSN is past `limit`. Every commit after it must be past
    /// `limit` too, or the target is not a prefix of the log.
    fn csn_cut(&self, records: &[WalRecord], limit: Csn) -> Result<Option<Lsn>> {
        let mut cut = None;
        for record in records {
            if record.kind != WalRecordKind::Commit || self.is_abandoned(record.lsn) {
                continue;
            }
            let WalPayload::Commit { csn, .. } = WalPayload::decode(&record.payload)? else {
                return Err(Error::CorruptWal("commit record has non-commit payload"));
            };
            match cut {
                None if csn > limit => cut = Some(record.lsn),
                Some(_) if csn <= limit => {
                    return Err(Error::CorruptWal(
                        "recovery target csn does not end a prefix of the wal",
                    ));
                }
                _ => {}
            }
        }
        Ok(cut)
    }

    fn is_abandoned(&self, lsn: Lsn) -> bool {
        self.abandoned
            .iter()
            .any(|(from, to)| *from <= lsn && lsn < *to)
    }

    /// Whether recovery applies the record at `lsn`.
    pub(super) fn applies(&self, lsn: Lsn) -> bool {
        self.cut.is_none_or(|cut| lsn < cut) && !self.is_abandoned(lsn)
    }

    /// Whether the commit record at `record_lsn` with `csn` is recovered.
    pub(super) fn commit_visible(&self, record_lsn: Lsn, csn: Csn) -> bool {
        self.applies(record_lsn)
            && match self.target {
                RecoveryTarget::Csn(limit) => csn <= limit,
                RecoveryTarget::Latest | RecoveryTarget::Lsn(_) => true,
            }
    }

    /// The ranges earlier forks abandoned, as recovery skipped them.
    pub(super) fn abandoned(&self) -> &[(Lsn, Lsn)] {
        &self.abandoned
    }

    /// The fork this recovery has to record for the next open to stay at
    /// its target: set when a record at or past the cut is still live.
    pub(super) fn fork_to_record(&self, records: &[WalRecord]) -> Option<PlannedFork> {
        let cut = self.cut?;
        records
            .iter()
            .any(|record| record.lsn >= cut && !self.is_abandoned(record.lsn))
            .then(|| PlannedFork {
                fork_lsn: cut,
                parent: TimelineId(self.forks + 1),
                child: TimelineId(self.forks + 2),
            })
    }
}

/// A timeline fork this recovery appends once it has succeeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PlannedFork {
    pub(super) fork_lsn: Lsn,
    pub(super) parent: TimelineId,
    pub(super) child: TimelineId,
}

impl PlannedFork {
    pub(super) fn payload(self) -> WalPayload {
        WalPayload::TimelineFork {
            parent: self.parent,
            fork_lsn: self.fork_lsn,
            child: self.child,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::TxId;

    fn record(lsn: u64, kind: WalRecordKind, payload: WalPayload) -> WalRecord {
        WalRecord {
            lsn: Lsn(lsn),
            prev_lsn: Lsn(lsn.saturating_sub(100)),
            tx_id: TxId(lsn),
            kind,
            payload: payload.encode().unwrap(),
        }
    }

    fn commit(lsn: u64, csn: u64) -> WalRecord {
        record(
            lsn,
            WalRecordKind::Commit,
            WalPayload::Commit {
                tx_id: TxId(lsn),
                csn: Csn(csn),
            },
        )
    }

    fn fork(lsn: u64, fork_lsn: u64) -> WalRecord {
        record(
            lsn,
            WalRecordKind::TimelineFork,
            WalPayload::TimelineFork {
                parent: TimelineId(1),
                fork_lsn: Lsn(fork_lsn),
                child: TimelineId(2),
            },
        )
    }

    #[test]
    fn records_a_fork_abandoned_are_skipped() {
        let records = [commit(0, 1), commit(100, 2), fork(200, 100), commit(300, 3)];
        let filter = ReplayFilter::new(&records, RecoveryTarget::Latest).unwrap();
        assert!(filter.applies(Lsn(0)));
        assert!(!filter.applies(Lsn(100)));
        assert!(filter.applies(Lsn(200)));
        assert!(filter.applies(Lsn(300)));
        assert_eq!(filter.abandoned(), &[(Lsn(100), Lsn(200))]);
        assert_eq!(filter.fork_to_record(&records), None);
    }

    #[test]
    fn a_csn_target_cuts_at_the_first_commit_past_it() {
        let records = [commit(0, 1), commit(100, 2), commit(200, 3)];
        let filter = ReplayFilter::new(&records, RecoveryTarget::Csn(Csn(2))).unwrap();
        assert!(filter.commit_visible(Lsn(100), Csn(2)));
        assert!(!filter.applies(Lsn(200)));
        let planned = filter.fork_to_record(&records).unwrap();
        assert_eq!(planned.fork_lsn, Lsn(200));
        assert_eq!(
            (planned.parent, planned.child),
            (TimelineId(1), TimelineId(2))
        );
    }

    #[test]
    fn a_csn_target_that_is_not_a_wal_prefix_fails() {
        let records = [commit(0, 1), commit(100, 3), commit(200, 2)];
        assert_eq!(
            ReplayFilter::new(&records, RecoveryTarget::Csn(Csn(2))).unwrap_err(),
            Error::CorruptWal("recovery target csn does not end a prefix of the wal")
        );
    }

    #[test]
    fn a_target_past_the_end_records_no_fork() {
        let records = [commit(0, 1), commit(100, 2)];
        let filter = ReplayFilter::new(&records, RecoveryTarget::Lsn(Lsn(500))).unwrap();
        assert_eq!(filter.fork_to_record(&records), None);
        let filter = ReplayFilter::new(&records, RecoveryTarget::Csn(Csn(9))).unwrap();
        assert_eq!(filter.fork_to_record(&records), None);
    }

    #[test]
    fn a_fork_that_names_a_later_lsn_fails() {
        let records = [commit(0, 1), fork(100, 150)];
        assert_eq!(
            ReplayFilter::new(&records, RecoveryTarget::Latest).unwrap_err(),
            Error::CorruptWal("timeline fork names an lsn after its own record")
        );
    }
}
