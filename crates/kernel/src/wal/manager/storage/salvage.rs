//! Keeping the bytes of a torn WAL tail (workplan R9, step 3).
//!
//! A crash in the middle of a write leaves the start of a record at the end
//! of the log. Recovery does not replay it, and the next record has to go
//! where it starts. Opening the WAL no longer cuts those bytes off. It
//! notes them as pending, and before the first write, or when recovery has
//! succeeded and asks, it copies them durably to
//! `wal/salvage/<segment>-<offset>.torn` and only then truncates the
//! segment. An open that fails leaves them where they were, and a tail that
//! was damaged history rather than an unfinished write can still be
//! examined afterwards.

use std::path::{Path, PathBuf};

use crate::io::{FileHandle, FileSystem, StdFileSystem, create_dir_all_durable};
use crate::{Error, Result};

use super::*;

/// The directory under `wal/` that holds salvaged tails.
pub const WAL_SALVAGE_DIR: &str = "salvage";

/// How many differently filled copies of one tail position to keep.
const MAX_SALVAGE_COPIES: u64 = 1000;

/// Bytes `[offset, len)` of `segment`, past where the WAL resumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingTail {
    pub(crate) segment: u64,
    pub(crate) offset: u64,
    pub(crate) len: u64,
}

/// Every run of bytes at or past where the WAL resumes: the rest of the
/// active segment, and any later segment that holds bytes. A writer that
/// rotated and died writing its first record in the new segment leaves the
/// tail in a segment after the one the log resumes in.
pub(super) fn pending_tails<Fs: FileSystem>(
    fs: &Fs,
    dir: &Path,
    active_segment: u64,
    active_offset: u64,
    active_len: u64,
) -> Result<Vec<PendingTail>> {
    if active_len < active_offset {
        return Err(Error::CorruptWal("wal resumes past the end of its segment"));
    }
    let mut tails = Vec::new();
    if active_len > active_offset {
        tails.push(PendingTail {
            segment: active_segment,
            offset: active_offset,
            len: active_len,
        });
    }
    for name in fs.read_dir_names(dir)? {
        let Some(segment) = parse_segment_name(&name) else {
            continue;
        };
        if segment <= active_segment {
            continue;
        }
        let len = fs.open_ro(&segment_path(dir, segment))?.len()?;
        if len > 0 {
            tails.push(PendingTail {
                segment,
                offset: 0,
                len,
            });
        }
    }
    tails.sort_by_key(|tail| tail.segment);
    Ok(tails)
}

/// Copy bytes `[from, to)` of `file`, which is WAL segment `segment`, to
/// the salvage directory under `wal_dir`, and make the copy and its name
/// durable. Returns the copy's path.
///
/// The copy is named `<segment>-<from>.torn`, both zero-padded to 20
/// digits. A file of that name that already holds these bytes, left by an
/// open that died before it truncated the segment, is kept and synced. One
/// that holds other bytes, from an earlier crash at the same position, is
/// kept too, and this copy takes the next free `<segment>-<from>.<n>.torn`.
pub(crate) fn salvage_bytes<Fs: FileSystem>(
    fs: &Fs,
    wal_dir: &Path,
    segment: u64,
    file: &mut Fs::File,
    from: u64,
    to: u64,
) -> Result<PathBuf> {
    let len = to
        .checked_sub(from)
        .ok_or_else(|| Error::CorruptWal("salvage range ends before it starts"))?;
    let len = usize::try_from(len).map_err(|_| Error::CorruptWal("salvage range too large"))?;
    let mut bytes = vec![0; len];
    file.read_exact_at(from, &mut bytes)?;
    let dir = wal_dir.join(WAL_SALVAGE_DIR);
    // A new directory is a new name in `wal/`.
    create_dir_all_durable(fs, &dir)?;
    let existing = fs.read_dir_names(&dir)?;
    for copy in 0..MAX_SALVAGE_COPIES {
        let name = if copy == 0 {
            format!("{segment:020}-{from:020}.torn")
        } else {
            format!("{segment:020}-{from:020}.{copy}.torn")
        };
        let path = dir.join(&name);
        if existing.contains(&name) {
            let mut found = fs.open_ro(&path)?;
            if found.len()? != bytes.len() as u64 {
                continue;
            }
            let mut held = vec![0; bytes.len()];
            found.read_exact_at(0, &mut held)?;
            if held != bytes {
                continue;
            }
            // The run that wrote it may have died before its sync.
            found.sync_data()?;
            fs.sync_dir(&dir)?;
            return Ok(path);
        }
        let mut out = fs.open_rw_create(&path)?;
        out.write_all_at(0, &bytes)?;
        out.sync_data()?;
        fs.sync_dir(&dir)?;
        return Ok(path);
    }
    Err(Error::CorruptWal(
        "too many salvaged copies of one wal tail",
    ))
}

/// Salvage every byte of `segment`, then empty it, durably. Recovery calls
/// this only for a torn segment below where the WAL resumes, which would
/// fail the next scan once a later segment exists.
pub(crate) fn salvage_and_empty_torn_segment(dir: &Path, segment: u64) -> Result<PathBuf> {
    let path = segment_path(dir, segment);
    let mut file = StdFileSystem.open_rw_existing(&path)?;
    let len = file.len()?;
    let salvaged = salvage_bytes(&StdFileSystem, dir, segment, &mut file, 0, len)?;
    file.set_len(0)?;
    file.sync_data()?;
    Ok(salvaged)
}

impl<Fs: FileSystem> WalManager<Fs> {
    /// Copy every pending tail to `wal/salvage/`, then cut it off: the
    /// active segment back to where the log resumes, a later segment to
    /// empty. Nothing is truncated until every copy is durable. Does
    /// nothing when no tail is pending.
    pub(crate) fn settle_torn_tail(&mut self) -> Result<()> {
        if self.pending_tails.is_empty() {
            return Ok(());
        }
        crate::failpoints::io_error_under("wal::salvage_error", &self.dir)?;
        let tails = self.pending_tails.clone();
        for tail in &tails {
            let path = if tail.segment == self.active_segment {
                salvage_bytes(
                    &self.fs,
                    &self.dir,
                    tail.segment,
                    &mut self.active_file,
                    tail.offset,
                    tail.len,
                )?
            } else {
                let mut file = self.fs.open_ro(&segment_path(&self.dir, tail.segment))?;
                salvage_bytes(
                    &self.fs,
                    &self.dir,
                    tail.segment,
                    &mut file,
                    tail.offset,
                    tail.len,
                )?
            };
            if !self.salvaged.contains(&path) {
                self.salvaged.push(path);
            }
        }
        for tail in &tails {
            if tail.segment == self.active_segment {
                self.active_file.set_len(tail.offset)?;
                self.active_file.sync_data()?;
            } else {
                let file = self
                    .fs
                    .open_rw_existing(&segment_path(&self.dir, tail.segment))?;
                file.set_len(0)?;
                file.sync_data()?;
            }
        }
        self.pending_tails.clear();
        Ok(())
    }

    /// The salvage files this manager has written or confirmed since it
    /// opened.
    pub fn salvaged_tails(&self) -> &[PathBuf] {
        &self.salvaged
    }
}
