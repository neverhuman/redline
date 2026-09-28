use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use crate::format::Lsn;
use crate::io::{FileSystem, StdFileSystem};
use crate::telemetry::Phase11Counters;
use crate::wal::{WAL_HEADER_LEN, WalRecord};
use crate::{Error, Result};

use super::counters::WalSyncCounters;
use super::*;

const WAL_SEGMENT_EXT: &str = ".wal";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalAppend {
    pub start_lsn: Lsn,
    pub end_lsn: Lsn,
}

#[derive(Debug)]
pub struct WalManager<Fs: FileSystem = StdFileSystem> {
    pub(crate) dir: PathBuf,
    pub(crate) fs: Fs,
    pub(crate) config: WalConfig,
    pub(crate) active_segment: u64,
    pub(crate) active_offset: u64,
    pub(crate) active_file: Fs::File,
    pub(crate) written_lsn: Lsn,
    pub(crate) durable_lsn: Lsn,
    pub(crate) prev_lsn: Lsn,
    /// Lane BH P1 #7: optional pointer to the coordinator's shared
    /// sync counters; left `None` for raw `WalManager` use (e.g.
    /// recovery scans) and populated when `WalCoordinator::new`
    /// hands ownership of the manager to the writer thread.
    pub(crate) sync_counters: Option<Arc<WalSyncCounters>>,
}

#[derive(Debug)]
pub struct WalCoordinator {
    pub(crate) shared: Arc<WalCoordinatorShared>,
    pub(crate) writer: Mutex<Option<JoinHandle<()>>>,
    pub(crate) config: WalConfig,
    pub(crate) dir: PathBuf,
    pub(crate) volatile: bool,
    /// Lane BH P1 #7: shared counters bumped by the writer thread.
    pub(crate) sync_counters: Arc<WalSyncCounters>,
}

#[derive(Debug)]
pub(crate) struct WalCoordinatorShared {
    pub(crate) state: Mutex<WalCoordinatorState>,
    pub(crate) cvar: Condvar,
    /// Wave 1A-F: optional Phase 11 telemetry sink, installed
    /// post-construction by [`WalCoordinator::set_phase11_counters`].
    /// The writer thread reads this directly (no state-mutex hop) so
    /// it can bump `wal_batch_size_buckets` per fdatasync.
    pub(crate) phase11: std::sync::RwLock<Option<Arc<Phase11Counters>>>,
    /// Lowest reserved LSNs of index updates whose WAL record is appended
    /// and whose page image is not installed yet. Checkpoint must not
    /// move past the minimum.
    pub(crate) install_fence: Mutex<BTreeMap<u64, u32>>,
}

#[derive(Debug)]
pub(crate) struct WalCoordinatorState {
    pub(crate) reserved_lsn: Lsn,
    pub(crate) written_lsn: Lsn,
    pub(crate) prev_lsn: Lsn,
    pub(crate) durable_lsn: Lsn,
    pub(crate) pending: VecDeque<QueuedWalRecord>,
    pub(crate) pending_bytes: usize,
    /// Durable wake predicate for the writer. Semantic-combiner candidates
    /// deliberately remain false until a flush, buffer-pressure write, or a
    /// non-combinable record releases the pending batch.
    pub(crate) write_requested: bool,
    pub(crate) flush_requested_lsn: Lsn,
    pub(crate) shutdown: bool,
    /// The first error the writer thread hit. The writer stops at it, so
    /// no record it had not made durable becomes durable afterwards.
    pub(crate) failure: Option<WalFailure>,
}

/// The WAL writer step that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalFailureStage {
    /// Writing queued records into the active segment.
    Write,
    /// The fsync that makes written records durable.
    Flush,
    /// Closing a full segment and creating the next one.
    Rotate,
}

impl std::fmt::Display for WalFailureStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Write => "write",
            Self::Flush => "flush",
            Self::Rotate => "rotate",
        })
    }
}

/// Why the WAL writer stopped. Records that end at or below `at_lsn` are
/// unaffected: written for `Write` and `Rotate`, durable for `Flush`.
/// Records past it may or may not have reached the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalFailure {
    pub stage: WalFailureStage,
    pub kind: std::io::ErrorKind,
    pub at_lsn: Lsn,
}

impl WalFailure {
    pub(crate) fn new(stage: WalFailureStage, err: &Error, at_lsn: Lsn) -> Self {
        let kind = match err {
            Error::Io(io) => io.kind(),
            _ => std::io::ErrorKind::Other,
        };
        Self {
            stage,
            kind,
            at_lsn,
        }
    }
}

impl From<WalFailure> for Error {
    fn from(failure: WalFailure) -> Self {
        Error::WalWriterFailed {
            stage: failure.stage,
            kind: failure.kind,
            at_lsn: failure.at_lsn,
        }
    }
}

#[derive(Debug)]
pub(crate) struct QueuedWalRecord {
    pub(crate) append: WalAppend,
    pub(crate) encoded: Vec<u8>,
}

#[derive(Debug)]
pub struct WalReader<Fs: FileSystem = StdFileSystem> {
    pub(crate) dir: PathBuf,
    pub(crate) fs: Fs,
    pub(crate) config: WalConfig,
    /// Report a whole record found after a torn tail in
    /// [`TornTail::valid_record_after`] instead of failing the scan.
    pub(crate) salvage_after_torn_tail: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalScanReport {
    pub records: Vec<WalRecord>,
    pub valid_end_lsn: Lsn,
    pub torn_tail: bool,
    /// Every segment file the scan found, in order, with its length.
    pub segments: Vec<WalSegmentInfo>,
    /// Position and back link of the first record, if any.
    pub first_record: Option<WalRecordLink>,
    /// Where and why the scan stopped before the end of the final segment.
    pub tail: Option<TornTail>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalSegmentInfo {
    pub number: u64,
    pub len: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalRecordLink {
    pub lsn: Lsn,
    pub prev_lsn: Lsn,
}

/// Bytes at the end of the final segment that do not hold a whole record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TornTail {
    pub segment: u64,
    pub offset: u64,
    pub file_len: u64,
    pub reason: TornTailReason,
    /// The offset of a whole, checksum-valid record at its own position
    /// after the tail. Set only by a scan that salvages; a strict scan
    /// fails instead.
    pub valid_record_after: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TornTailReason {
    PartialHeader,
    LengthOverflow,
    LengthExceedsSegment,
    PartialBody,
    UndecodableRecord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WalOpenScanSummary {
    pub(crate) valid_end_lsn: Lsn,
    pub(crate) last_record_lsn: Lsn,
}

impl WalOpenScanSummary {
    /// Resume a WAL that holds no records at `floor`, rounded up to the next
    /// segment boundary unless it already sits on one.
    ///
    /// Opening mid-segment would extend the new segment with zeros up to the
    /// offset, and the next scan reads those zeros as a torn record. No record
    /// precedes the first new one, so its previous LSN is zero; the scan
    /// accepts any previous LSN on the first record.
    pub(crate) fn at_floor(floor: Lsn, segment_bytes: u64) -> Result<Self> {
        if segment_bytes == 0 {
            return Err(Error::CorruptWal("wal segment size too small"));
        }
        let start = floor
            .0
            .div_ceil(segment_bytes)
            .checked_mul(segment_bytes)
            .ok_or(Error::CorruptWal("lsn overflow"))?;
        Ok(Self {
            valid_end_lsn: Lsn(start),
            last_record_lsn: Lsn::ZERO,
        })
    }
}

impl WalScanReport {
    pub(crate) fn open_summary(&self) -> WalOpenScanSummary {
        WalOpenScanSummary {
            valid_end_lsn: self.valid_end_lsn,
            last_record_lsn: self
                .records
                .last()
                .map(|record| record.lsn)
                .unwrap_or(Lsn::ZERO),
        }
    }
}

pub(super) fn validate_config(config: &WalConfig) -> Result<()> {
    if config.segment_bytes < WAL_HEADER_LEN as u64 {
        return Err(Error::CorruptWal("wal segment size too small"));
    }
    if config.wal_buffer_bytes < WAL_HEADER_LEN {
        return Err(Error::CorruptWal("wal buffer too small"));
    }
    if config.wal_write_batch_bytes == 0 {
        return Err(Error::CorruptWal("wal write batch must be nonzero"));
    }
    Ok(())
}

pub(super) fn validate_record_position(
    record: &WalRecord,
    segment: u64,
    offset: u64,
    segment_bytes: u64,
) -> Result<()> {
    let expected = segment
        .checked_sub(1)
        .and_then(|index| index.checked_mul(segment_bytes))
        .and_then(|base| base.checked_add(offset))
        .ok_or(Error::CorruptWal("lsn overflow"))?;
    if record.lsn.0 != expected {
        return Err(Error::CorruptWal(
            "record lsn does not match segment position",
        ));
    }
    Ok(())
}

pub(super) fn segment_for_lsn(lsn: Lsn, segment_bytes: u64) -> u64 {
    lsn.0 / segment_bytes + 1
}

pub(super) fn offset_for_lsn(lsn: Lsn, segment_bytes: u64) -> u64 {
    lsn.0 % segment_bytes
}

pub(super) fn segment_path(dir: &Path, segment: u64) -> PathBuf {
    dir.join(format!("{segment:020}{WAL_SEGMENT_EXT}"))
}

pub(super) fn segment_numbers_on_disk(dir: &Path) -> Result<Vec<u64>> {
    let mut segments = Vec::new();
    if !dir.exists() {
        return Ok(segments);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if let Some(name) = entry.file_name().to_str()
            && let Some(segment) = parse_segment_name(name)
        {
            segments.push(segment);
        }
    }
    segments.sort_unstable();
    Ok(segments)
}

/// Segment numbers on disk, in order, each with whether its file holds any
/// bytes.
pub(crate) fn segments_on_disk_with_bytes(dir: &Path) -> Result<Vec<(u64, bool)>> {
    segment_numbers_on_disk(dir)?
        .into_iter()
        .map(|segment| {
            let len = std::fs::metadata(segment_path(dir, segment))?.len();
            Ok((segment, len > 0))
        })
        .collect()
}

/// Truncate a segment whose bytes a scan read as a torn tail, durably.
/// Recovery calls this only for a segment the WAL resumes past.
pub(crate) fn empty_torn_segment(dir: &Path, segment: u64) -> Result<()> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(segment_path(dir, segment))?;
    file.set_len(0)?;
    file.sync_all()?;
    Ok(())
}

pub(super) fn parse_segment_name(name: &str) -> Option<u64> {
    let number = name.strip_suffix(WAL_SEGMENT_EXT)?;
    if number.len() != 20 || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    number.parse().ok()
}
