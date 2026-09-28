use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::format::Lsn;
use crate::format::bytes::{crc32_with_zeroed_field, read_u32, read_u64, write_u32, write_u64};
use crate::storage::sync_parent_dir;
use crate::{Error, Result};

pub const CONTROL_MAGIC: u32 = 0x5244_4354; // "RDCT"
/// Version 2 adds `heap_redo_lsn`. A build that knows only version 1 refuses
/// the file instead of replaying heap records the page file already holds.
pub const CONTROL_VERSION: u32 = 2;
/// Files written before `heap_redo_lsn` existed. Their heap redo starts at
/// the checkpoint LSN.
const CONTROL_VERSION_V1: u32 = 1;
pub const CONTROL_LEN: usize = 64;

const CONTROL_A: &str = "CONTROL_A";
const CONTROL_B: &str = "CONTROL_B";
const CHECKSUM_OFFSET: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ControlFile {
    pub generation: u64,
    /// Where recovery starts replaying the WAL, and the LSN below which a
    /// checkpoint may prune it.
    pub checkpoint_lsn: Lsn,
    pub page_count: u64,
    /// Where heap redo starts; never below `checkpoint_lsn`. The checkpoint
    /// wrote its heap pages while no logged heap change was in flight, so
    /// they hold every heap record below this LSN and none at or above it.
    /// Heap redo appends a replayed row as a new version rather than onto
    /// the page it changed, so replaying a heap record the page file already
    /// holds would leave a second copy of the row.
    pub heap_redo_lsn: Lsn,
    /// A version-2 checkpoint wrote every dirty page, so a heap page past
    /// `page_count` holds nothing below `heap_redo_lsn`. A version-1 file
    /// (v4.1.0) makes no such promise: its checkpoint could skip dirty
    /// pages that eviction wrote later.
    pub complete_cut: bool,
}

#[derive(Debug)]
pub struct ControlStore {
    dir: PathBuf,
}

/// What the two control slots hold.
#[derive(Debug, Default, PartialEq)]
pub struct ControlSelection {
    /// The checksum-valid control file with the highest generation.
    pub newest: Option<ControlFile>,
    /// The other slot's control file, when it is checksum-valid too.
    pub fallback: Option<ControlFile>,
    /// Slots holding a file that does not decode. Such a slot may have held
    /// the newest generation before a torn write or damage.
    pub corrupt_slots: Vec<CorruptControlSlot>,
}

#[derive(Debug, PartialEq)]
pub struct CorruptControlSlot {
    pub name: &'static str,
    pub error: Error,
}

impl ControlStore {
    pub fn new(dir: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(dir.as_ref())?;
        Ok(Self {
            dir: dir.as_ref().to_path_buf(),
        })
    }

    /// Like [`new`] but skips the `create_dir_all` call.  Use for volatile
    /// in-memory databases where the caller has already created the directory
    /// (saves 2–3 redundant syscalls per process start).
    pub(crate) fn new_volatile(dir: impl AsRef<Path>) -> Self {
        Self {
            dir: dir.as_ref().to_path_buf(),
        }
    }

    /// The newest checksum-valid control file, or `None` when neither slot
    /// holds a file. A slot that holds a file that does not decode, with no
    /// valid slot beside it, is an error: that database was checkpointed,
    /// and replaying as if it never was would read a pruned WAL.
    pub fn load_latest(&self) -> Result<Option<ControlFile>> {
        let selection = self.load_selection()?;
        match (selection.newest, selection.corrupt_slots.into_iter().next()) {
            (Some(newest), _) => Ok(Some(newest)),
            (None, Some(corrupt)) => Err(corrupt.error),
            (None, None) => Ok(None),
        }
    }

    /// Both slots, sorted by generation, and the slots that hold a file that
    /// does not decode. Recovery chooses among them (workplan R6): a missing
    /// slot and a corrupt one mean different things. An error reading a
    /// slot, other than its absence, fails the load rather than passing for
    /// corruption.
    pub fn load_selection(&self) -> Result<ControlSelection> {
        let mut selection = ControlSelection::default();
        let mut valid = Vec::with_capacity(2);
        for name in [CONTROL_A, CONTROL_B] {
            match self.read_named(name) {
                Ok(Some(control)) => valid.push(control),
                Ok(None) => {}
                Err(Error::Io(err)) => return Err(Error::Io(err)),
                // A slot with this magic and a version this build does not
                // know was written by a newer build, not damaged. Recovering
                // past it, from the other slot or from the WAL alone, would
                // replay heap records the newer build's page file holds.
                Err(Error::UnsupportedVersion(version)) => {
                    return Err(Error::UnsupportedVersion(version));
                }
                Err(error) => selection
                    .corrupt_slots
                    .push(CorruptControlSlot { name, error }),
            }
        }
        valid.sort_by_key(|control| std::cmp::Reverse(control.generation));
        let mut valid = valid.into_iter();
        selection.newest = valid.next();
        selection.fallback = valid.next();
        Ok(selection)
    }

    /// Write the generation after `previous`. Its redo LSNs may not be below
    /// the previous generation's: that one may already have pruned the WAL
    /// below them, and recovery starts from the newest generation.
    pub fn write_next(
        &self,
        previous: Option<ControlFile>,
        checkpoint_lsn: Lsn,
        heap_redo_lsn: Lsn,
        page_count: u64,
    ) -> Result<ControlFile> {
        if heap_redo_lsn < checkpoint_lsn {
            return Err(Error::CorruptPage(
                "checkpoint heap redo lsn is below its checkpoint lsn",
            ));
        }
        if let Some(previous) = previous
            && (checkpoint_lsn < previous.checkpoint_lsn || heap_redo_lsn < previous.heap_redo_lsn)
        {
            return Err(Error::CorruptPage(
                "checkpoint redo lsn is below the previous generation's",
            ));
        }
        let next = ControlFile {
            generation: previous.map(|control| control.generation + 1).unwrap_or(1),
            checkpoint_lsn,
            page_count,
            heap_redo_lsn,
            complete_cut: true,
        };
        let name = if next.generation.is_multiple_of(2) {
            CONTROL_B
        } else {
            CONTROL_A
        };
        self.write_named(name, &next)?;
        Ok(next)
    }

    fn read_named(&self, name: &str) -> Result<Option<ControlFile>> {
        let path = self.dir.join(name);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(path)?;
        Ok(Some(ControlFile::decode(&bytes)?))
    }

    fn write_named(&self, name: &str, control: &ControlFile) -> Result<()> {
        // Lane E failpoint: armed before the control-file generation lands on
        // disk. Crashing here forces the dual-control-file recovery path to
        // fall back to the previous generation's checksum-valid image.
        crate::fail_point!("storage::control::write");
        let path = self.dir.join(name);
        let bytes = control.encode()?;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)?;
        file.write_all(&bytes)?;
        file.sync_data()?;
        sync_parent_dir(&self.dir)?;
        Ok(())
    }
}

impl ControlFile {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = vec![0; CONTROL_LEN];
        write_u32(&mut out, 0, CONTROL_MAGIC)?;
        write_u32(&mut out, 4, CONTROL_VERSION)?;
        write_u32(&mut out, CHECKSUM_OFFSET, 0)?;
        write_u32(&mut out, 12, 0)?;
        write_u64(&mut out, 16, self.generation)?;
        write_u64(&mut out, 24, self.checkpoint_lsn.0)?;
        write_u64(&mut out, 32, self.page_count)?;
        write_u64(&mut out, 40, self.heap_redo_lsn.0)?;
        let checksum = checksum_control_bytes(&out);
        write_u32(&mut out, CHECKSUM_OFFSET, checksum)?;
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != CONTROL_LEN {
            return Err(Error::BufferTooSmall {
                needed: CONTROL_LEN,
                actual: bytes.len(),
            });
        }
        let magic = read_u32(bytes, 0)?;
        if magic != CONTROL_MAGIC {
            return Err(Error::InvalidMagic {
                expected: CONTROL_MAGIC,
                actual: magic,
            });
        }
        let version = read_u32(bytes, 4)?;
        if version != CONTROL_VERSION && version != CONTROL_VERSION_V1 {
            return Err(Error::UnsupportedVersion(version as u16));
        }
        let stored = read_u32(bytes, CHECKSUM_OFFSET)?;
        let actual = checksum_control_bytes(bytes);
        if stored != actual {
            return Err(Error::InvalidChecksum);
        }
        let checkpoint_lsn = Lsn(read_u64(bytes, 24)?);
        let heap_redo_lsn = if version == CONTROL_VERSION_V1 {
            checkpoint_lsn
        } else {
            Lsn(read_u64(bytes, 40)?)
        };
        if heap_redo_lsn < checkpoint_lsn {
            return Err(Error::CorruptPage(
                "control file heap redo lsn is below its checkpoint lsn",
            ));
        }
        Ok(Self {
            generation: read_u64(bytes, 16)?,
            checkpoint_lsn,
            page_count: read_u64(bytes, 32)?,
            heap_redo_lsn,
            complete_cut: version != CONTROL_VERSION_V1,
        })
    }
}

fn checksum_control_bytes(bytes: &[u8]) -> u32 {
    crc32_with_zeroed_field(bytes, CHECKSUM_OFFSET)
}
