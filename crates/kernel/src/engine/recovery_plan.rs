//! Which checkpoint generation recovery starts from, and whether the WAL
//! holds everything that start needs (workplan R6, and R9 steps 1-2).
//!
//! Recovery reads the control files, the chosen generation's transaction
//! status and the catalog, and checks the scanned WAL against them, before
//! it changes any file. A check that fails here fails the open with the
//! database exactly as it was.
//!
//! The WAL is whole for a start LSN when:
//! - no segment is missing from the one holding the first record through
//!   the last one on disk (the scan already checks that each record names
//!   the one before it);
//! - with no checkpoint, the first record is the log's first, at LSN 0;
//! - with a checkpoint at C, the first record is at or below C, or the
//!   record before it (by its previous LSN) was below C, so every record
//!   from C on is still there; and the WAL reaches the checkpoint's heap
//!   redo LSN. A WAL with no records at all is left to the append floor
//!   (`recovery_wal_floor.rs`), which fails unless no segment below the
//!   checkpoint remains.

use std::sync::Arc;

use crate::catalog::{SchemaSnapshot, bootstrap_schema};
use crate::format::{Lsn, RelId};
use crate::storage::{ControlFile, ControlSelection, TxStatusCheckpoint, TxStatusStore};
use crate::wal::WalScanReport;
use crate::{Error, Result};

const NO_START: Error =
    Error::CorruptWal("wal does not start at lsn 0 and no checkpoint covers the records before it");
const NO_VALID_CONTROL: Error =
    Error::CorruptWal("no valid control file and the wal does not start at lsn 0");
const FALLBACK_LACKS_WAL: Error = Error::CorruptWal("fallback checkpoint lacks required WAL");

/// Where replay starts, once the WAL is known to hold everything from there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RecoveryPlan {
    /// Index, page image and commit records replay from here.
    pub(super) replay_from_lsn: Lsn,
    /// Heap records replay from here; never below `replay_from_lsn`.
    pub(super) heap_replay_from: Lsn,
    /// Heap pages up to here hold every heap record below
    /// `heap_replay_from`; pages above it hold only what replay rewrites.
    pub(super) heap_page_count: u64,
}

/// The generation recovery starts from, with what it loaded to choose it.
#[derive(Debug)]
pub(super) struct GenerationChoice {
    pub(super) checkpoint: Option<ControlFile>,
    pub(super) tx_status: Option<TxStatusCheckpoint>,
    pub(super) plan: RecoveryPlan,
    /// A checksum-valid newer generation that recovery could not use.
    pub(super) skipped_generation: Option<u64>,
    pub(super) warnings: Vec<String>,
}

/// Where a checkpoint's replay has to reach: its checkpoint LSN or, past
/// that, its heap redo LSN. The checkpoint made the WAL durable through both
/// before it wrote the control file.
fn redo_end(checkpoint: &ControlFile) -> Lsn {
    checkpoint.checkpoint_lsn.max(checkpoint.heap_redo_lsn)
}

/// Check that `scan` holds every record recovery from `checkpoint` needs.
pub(super) fn validate_recovery_plan(
    scan: &WalScanReport,
    checkpoint: Option<&ControlFile>,
    segment_bytes: u64,
) -> Result<RecoveryPlan> {
    if segment_bytes == 0 {
        return Err(Error::CorruptWal("wal segment size too small"));
    }
    if let Some(first) = scan.first_record {
        // The scan fails on a gap between two records, but not on one that
        // no later record crosses, such as before a final segment that
        // holds only a torn tail. Segments below the first record's hold no
        // bytes (the scan fails on bytes that are not whole records there).
        let first_segment = first.lsn.0 / segment_bytes + 1;
        let contiguous = scan
            .segments
            .iter()
            .filter(|info| info.number >= first_segment)
            .zip(first_segment..)
            .all(|(info, expected)| info.number == expected);
        if !contiguous {
            return Err(Error::CorruptWal(
                "wal segment missing between retained segments",
            ));
        }
    }
    let segment_start = |number: u64| Lsn(number.saturating_sub(1).saturating_mul(segment_bytes));
    match (checkpoint, scan.first_record) {
        (None, Some(first)) => {
            if first.lsn != Lsn::ZERO || first.prev_lsn != Lsn::ZERO {
                return Err(NO_START);
            }
        }
        // Nothing prunes the WAL before the first checkpoint, and a segment
        // past the first exists only once records filled the one before.
        (None, None) => {
            if scan.segments.iter().any(|info| info.number > 1) {
                return Err(NO_START);
            }
        }
        (Some(checkpoint), Some(first)) => {
            if first.lsn > checkpoint.checkpoint_lsn && first.prev_lsn >= checkpoint.checkpoint_lsn
            {
                return Err(Error::CorruptWal(
                    "wal starts after the checkpoint redo lsn",
                ));
            }
            if scan.valid_end_lsn < redo_end(checkpoint) {
                return Err(Error::CorruptWal("wal ends before checkpoint redo lsn"));
            }
        }
        (Some(checkpoint), None) => {
            let end = redo_end(checkpoint);
            if scan
                .segments
                .iter()
                .any(|info| segment_start(info.number) < end)
            {
                return Err(Error::CorruptWal("wal ends before checkpoint redo lsn"));
            }
        }
    }
    Ok(RecoveryPlan {
        replay_from_lsn: checkpoint.map_or(Lsn::ZERO, |checkpoint| checkpoint.checkpoint_lsn),
        heap_replay_from: checkpoint.map_or(Lsn::ZERO, redo_end),
        heap_page_count: checkpoint.map_or(0, |checkpoint| checkpoint.page_count),
    })
}

/// Choose the newest generation recovery can start from.
///
/// A generation qualifies when its transaction status file loads with its
/// own generation and the WAL holds everything replay from it needs. When
/// the newest checksum-valid generation does not qualify, the other slot's
/// may: the checkpoint that wrote the newest kept the WAL the older one
/// replays. Recovering from an older generation replays over a page file
/// that may hold what a newer checkpoint wrote, so it also needs WAL
/// records through the newer generation's redo end: only they say which of
/// those changes committed.
///
/// A slot that does not decode may be a newer generation whose control
/// write tore, so the valid slot beside it is held to the fallback rule. With
/// no valid slot at all, recovery replays the whole WAL, which must then
/// start at LSN 0.
pub(super) fn select_recoverable_generation(
    selection: &ControlSelection,
    scan: &WalScanReport,
    tx_status_store: &TxStatusStore,
    segment_bytes: u64,
) -> Result<GenerationChoice> {
    let mut warnings: Vec<String> = selection
        .corrupt_slots
        .iter()
        .map(|slot| format!("control file {} is corrupt: {}", slot.name, slot.error))
        .collect();
    let corrupt_slot = !selection.corrupt_slots.is_empty();
    let Some(newest) = selection.newest else {
        // A corrupt slot says a checkpoint ran, and may have pruned the
        // WAL below it; say so rather than only that the WAL is short.
        let plan = validate_recovery_plan(scan, None, segment_bytes).map_err(|err| {
            if corrupt_slot && err == NO_START {
                NO_VALID_CONTROL
            } else {
                err
            }
        })?;
        if corrupt_slot {
            warnings.push("no control file is valid; replaying the whole wal".to_owned());
        }
        return Ok(GenerationChoice {
            checkpoint: None,
            tx_status: None,
            plan,
            skipped_generation: None,
            warnings,
        });
    };
    let newer = if corrupt_slot {
        Newer::CorruptSlot
    } else {
        Newer::Nothing
    };
    let newest_err = match load_candidate(&newest, newer, scan, tx_status_store, segment_bytes) {
        Ok((tx_status, plan)) => {
            return Ok(GenerationChoice {
                checkpoint: Some(newest),
                tx_status: Some(tx_status),
                plan,
                skipped_generation: None,
                warnings,
            });
        }
        Err(err) => err,
    };
    if let Some(fallback) = selection.fallback
        && let Ok((tx_status, mut plan)) = load_candidate(
            &fallback,
            Newer::Valid(&newest),
            scan,
            tx_status_store,
            segment_bytes,
        )
    {
        // The newer checkpoint wrote and synced its pages before its control
        // file, and nothing writes a logged heap page between checkpoints. So
        // the heap in the file is the newer generation's: heap redo starts at
        // its heap redo LSN, or it would add a second copy of every row the
        // newer checkpoint wrote. Commits still come from this generation's
        // status and the WAL after it.
        plan.heap_replay_from = plan.heap_replay_from.max(redo_end(&newest));
        plan.heap_page_count = newest.page_count;
        warnings.push(format!(
            "checkpoint generation {} is unusable ({newest_err}); recovered from generation {} \
             with the heap of generation {}",
            newest.generation, fallback.generation, newest.generation
        ));
        return Ok(GenerationChoice {
            checkpoint: Some(fallback),
            tx_status: Some(tx_status),
            plan,
            skipped_generation: Some(newest.generation),
            warnings,
        });
    }
    // Beside a corrupt slot, the valid one is a fallback, and what it lacks
    // is WAL a newer generation's pages may depend on.
    if corrupt_slot && matches!(newest_err, Error::CorruptWal(_)) {
        return Err(FALLBACK_LACKS_WAL);
    }
    Err(newest_err)
}

/// What may have written the page file after a candidate generation.
#[derive(Clone, Copy)]
enum Newer<'a> {
    Nothing,
    /// A slot that does not decode, of unknown generation.
    CorruptSlot,
    /// A checksum-valid newer generation.
    Valid(&'a ControlFile),
}

/// Load `candidate`'s transaction status and check the WAL covers it.
fn load_candidate(
    candidate: &ControlFile,
    newer: Newer<'_>,
    scan: &WalScanReport,
    tx_status_store: &TxStatusStore,
    segment_bytes: u64,
) -> Result<(TxStatusCheckpoint, RecoveryPlan)> {
    let tx_status = tx_status_store.load(candidate.generation)?;
    if tx_status.generation != candidate.generation {
        return Err(Error::CorruptPage(
            "tx status checkpoint generation mismatch",
        ));
    }
    let plan = validate_recovery_plan(scan, Some(candidate), segment_bytes)?;
    // An empty WAL cannot say which changes a newer checkpoint wrote to the
    // page file after this one.
    let covers_newer = match newer {
        Newer::Nothing => true,
        Newer::CorruptSlot => !scan.records.is_empty(),
        Newer::Valid(newer) => !scan.records.is_empty() && scan.valid_end_lsn >= redo_end(newer),
    };
    if !covers_newer {
        return Err(FALLBACK_LACKS_WAL);
    }
    Ok((tx_status, plan))
}

/// The catalog recovery starts from.
///
/// A catalog snapshot in the WAL is the newest committed schema: the WAL
/// holds a suffix of the log, and every later schema change would log a
/// snapshot after it. Without one, the schema file must load. A file that
/// is missing is the empty bootstrap schema only for a database that holds
/// nothing yet: no checkpoint, which would have saved the file, and no page
/// data. Anything else would silently drop every table.
pub(super) fn resolve_catalog(
    file: Result<Option<Arc<SchemaSnapshot>>>,
    wal_snapshot: Option<Arc<SchemaSnapshot>>,
    holds_data: bool,
) -> Result<Arc<SchemaSnapshot>> {
    match (file, wal_snapshot) {
        (_, Some(snapshot)) => Ok(snapshot),
        (Ok(Some(snapshot)), None) => Ok(snapshot),
        (Err(Error::Io(err)), None) => Err(Error::Io(err)),
        (Err(_), None) => Err(Error::CorruptPage(
            "catalog file is unreadable and the wal holds no catalog snapshot",
        )),
        (Ok(None), None) if !holds_data => Ok(bootstrap_schema(RelId(10_000))),
        (Ok(None), None) => Err(Error::CorruptPage(
            "catalog file is missing and the wal holds no catalog snapshot",
        )),
    }
}

#[cfg(test)]
#[path = "recovery_plan_tests.rs"]
mod tests;
