//! Directory durability of WAL segment names.
//!
//! A new segment's name lives in the WAL directory. `sync_data` on the
//! segment file does not make that entry durable, so the directory has to
//! be fsynced after the create and before any record in the segment is
//! acknowledged. These tests record every create, write, data sync and
//! directory sync through a wrapping [`FileSystem`] and check the order.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tempfile::TempDir;

use super::*;
use crate::io::StdFileHandle;
use crate::wal::WalReader;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Op {
    Create(PathBuf),
    Write(PathBuf),
    SyncData(PathBuf),
    SyncDir(PathBuf),
    SetLen(PathBuf, u64),
}

/// [`StdFileSystem`] plus a shared log of the operations that matter for
/// name durability, and a switch that makes every directory sync fail.
#[derive(Clone, Debug, Default)]
struct RecordingFs {
    log: Arc<Mutex<Vec<Op>>>,
    fail_sync_dir: Arc<AtomicBool>,
}

impl RecordingFs {
    fn log(&self) -> Vec<Op> {
        self.log.lock().unwrap().clone()
    }

    fn fail_sync_dir(&self, fail: bool) {
        self.fail_sync_dir.store(fail, Ordering::SeqCst);
    }

    fn record(&self, op: Op) {
        self.log.lock().unwrap().push(op);
    }
}

#[derive(Debug)]
struct RecordingFile {
    inner: StdFileHandle,
    path: PathBuf,
    fs: RecordingFs,
}

impl FileSystem for RecordingFs {
    type File = RecordingFile;

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        StdFileSystem.create_dir_all(path)
    }

    fn read_dir_names(&self, path: &Path) -> Result<Vec<String>> {
        StdFileSystem.read_dir_names(path)
    }

    fn open_rw_create(&self, path: &Path) -> Result<Self::File> {
        let inner = StdFileSystem.open_rw_create(path)?;
        self.record(Op::Create(path.to_path_buf()));
        Ok(RecordingFile {
            inner,
            path: path.to_path_buf(),
            fs: self.clone(),
        })
    }

    fn open_rw_existing(&self, path: &Path) -> Result<Self::File> {
        Ok(RecordingFile {
            inner: StdFileSystem.open_rw_existing(path)?,
            path: path.to_path_buf(),
            fs: self.clone(),
        })
    }

    fn open_ro(&self, path: &Path) -> Result<Self::File> {
        Ok(RecordingFile {
            inner: StdFileSystem.open_ro(path)?,
            path: path.to_path_buf(),
            fs: self.clone(),
        })
    }

    fn sync_dir(&self, path: &Path) -> Result<()> {
        if self.fail_sync_dir.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("injected directory fsync failure").into());
        }
        StdFileSystem.sync_dir(path)?;
        self.record(Op::SyncDir(path.to_path_buf()));
        Ok(())
    }
}

impl FileHandle for RecordingFile {
    fn len(&self) -> Result<u64> {
        self.inner.len()
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.inner.read_exact_at(offset, buf)
    }

    fn write_all_at(&mut self, offset: u64, buf: &[u8]) -> Result<()> {
        self.inner.write_all_at(offset, buf)?;
        self.fs.record(Op::Write(self.path.clone()));
        Ok(())
    }

    fn sync_data(&self) -> Result<()> {
        self.inner.sync_data()?;
        self.fs.record(Op::SyncData(self.path.clone()));
        Ok(())
    }

    fn set_len(&self, len: u64) -> Result<()> {
        self.inner.set_len(len)?;
        self.fs.record(Op::SetLen(self.path.clone(), len));
        Ok(())
    }
}

/// Header is 48 bytes, so a 16-byte payload makes a 64-byte record and two
/// records fill a 128-byte segment. The third append rotates.
fn config() -> WalConfig {
    WalConfig {
        segment_bytes: 128,
        ..WalConfig::default()
    }
}

fn append(manager: &mut WalManager<impl FileSystem>) -> Result<WalAppend> {
    manager.append(WalRecordKind::PageDelta, TxId(1), vec![b'x'; 16])
}

fn position(log: &[Op], op: &Op) -> usize {
    position_after(log, 0, op)
}

fn position_after(log: &[Op], start: usize, op: &Op) -> usize {
    log.iter()
        .skip(start)
        .position(|seen| seen == op)
        .map(|index| index + start)
        .unwrap_or_else(|| panic!("{op:?} missing at or after {start} in {log:#?}"))
}

/// The directory sync that follows the create of `segment` comes after it
/// and before the first write to it, and that write is data-synced.
fn assert_name_synced_before_write(log: &[Op], dir: &Path, segment: &Path) {
    let created = position(log, &Op::Create(segment.to_path_buf()));
    let synced = position_after(log, created, &Op::SyncDir(dir.to_path_buf()));
    let written = position(log, &Op::Write(segment.to_path_buf()));
    let flushed = position_after(log, written, &Op::SyncData(segment.to_path_buf()));
    assert!(
        created < synced && synced < written && written < flushed,
        "want Create < SyncDir(dir) < Write < SyncData for {}: {log:#?}",
        segment.display()
    );
}

#[test]
fn first_segment_name_synced_before_first_flush() {
    let root = TempDir::new().unwrap();
    let wal_dir = root.path().join("wal");
    let fs = RecordingFs::default();
    let mut manager = WalManager::create_with_fs(&wal_dir, config(), fs.clone()).unwrap();
    append(&mut manager).unwrap();
    manager.flush().unwrap();

    let log = fs.log();
    assert_name_synced_before_write(&log, &wal_dir, &segment_path(&wal_dir, 1));
    // `wal` itself is a new name in the database root.
    let root_synced = position(&log, &Op::SyncDir(root.path().to_path_buf()));
    let created = position(&log, &Op::Create(segment_path(&wal_dir, 1)));
    assert!(root_synced < created, "{log:#?}");
}

#[test]
fn creating_nested_wal_dirs_syncs_each_new_parent_outermost_first() {
    let root = TempDir::new().unwrap();
    let outer = root.path().join("a");
    let inner = outer.join("b");
    let wal_dir = inner.join("wal");
    let fs = RecordingFs::default();
    let manager = WalManager::create_with_fs(&wal_dir, config(), fs.clone()).unwrap();
    drop(manager);

    let log = fs.log();
    let root_synced = position(&log, &Op::SyncDir(root.path().to_path_buf()));
    let outer_synced = position(&log, &Op::SyncDir(outer.clone()));
    let inner_synced = position(&log, &Op::SyncDir(inner.clone()));
    let wal_synced = position(&log, &Op::SyncDir(wal_dir.clone()));
    assert!(
        root_synced < outer_synced && outer_synced < inner_synced && inner_synced < wal_synced,
        "{log:#?}"
    );

    // Reopening finds every directory present and syncs none of the parents.
    let reopen = RecordingFs::default();
    drop(WalManager::open_with_fs(&wal_dir, config(), reopen.clone()).unwrap());
    let log = reopen.log();
    for parent in [root.path(), outer.as_path(), inner.as_path()] {
        assert!(
            !log.contains(&Op::SyncDir(parent.to_path_buf())),
            "{} synced again on reopen: {log:#?}",
            parent.display()
        );
    }
}

#[test]
fn rotation_syncs_dir_before_writing_new_segment() {
    let dir = TempDir::new().unwrap();
    let fs = RecordingFs::default();
    let mut manager = WalManager::create_with_fs(dir.path(), config(), fs.clone()).unwrap();
    append(&mut manager).unwrap();
    append(&mut manager).unwrap();
    let rotated = append(&mut manager).unwrap();
    assert_eq!(rotated.start_lsn, Lsn(128));
    manager.flush().unwrap();

    assert_name_synced_before_write(&fs.log(), dir.path(), &segment_path(dir.path(), 2));
}

#[test]
fn dir_sync_failure_rejects_append_and_keeps_segment() {
    let dir = TempDir::new().unwrap();
    let fs = RecordingFs::default();
    let mut manager = WalManager::create_with_fs(dir.path(), config(), fs.clone()).unwrap();
    append(&mut manager).unwrap();
    append(&mut manager).unwrap();

    fs.fail_sync_dir(true);
    let err = append(&mut manager)
        .expect_err("an append into a segment whose name is not durable must fail");
    assert!(matches!(err, Error::Io(_)), "{err:?}");
    // The manager stays on the old segment and writes nothing new.
    assert_eq!(manager.active_segment, 1);
    assert_eq!(manager.written_lsn(), Lsn(128));
    let segment2 = segment_path(dir.path(), 2);
    assert!(
        !fs.log().contains(&Op::Write(segment2.clone())),
        "{:#?}",
        fs.log()
    );

    // Once the directory sync works again the next append rotates cleanly.
    fs.fail_sync_dir(false);
    let retried = append(&mut manager).unwrap();
    assert_eq!(retried.start_lsn, Lsn(128));
    assert_eq!(manager.active_segment, 2);
    manager.flush().unwrap();
    assert_name_synced_before_write(&fs.log(), dir.path(), &segment2);
    drop(manager);

    let lsns: Vec<Lsn> = WalReader::new(dir.path(), config())
        .scan()
        .unwrap()
        .iter()
        .map(|record| record.lsn)
        .collect();
    assert_eq!(lsns, vec![Lsn(0), Lsn(64), Lsn(128)]);
}

#[test]
fn open_at_exact_segment_boundary_syncs_new_segment() {
    let dir = TempDir::new().unwrap();
    let mut first = WalManager::create(dir.path(), config()).unwrap();
    append(&mut first).unwrap();
    append(&mut first).unwrap();
    first.flush().unwrap();
    drop(first);
    let segment2 = segment_path(dir.path(), 2);
    assert!(!segment2.exists());

    // The valid WAL ends exactly on the segment boundary, so open creates
    // segment 2 as the active segment.
    let fs = RecordingFs::default();
    let mut manager = WalManager::open_with_fs(dir.path(), config(), fs.clone()).unwrap();
    assert_eq!(manager.active_segment, 2);
    assert_eq!(manager.written_lsn(), Lsn(128));
    let next = append(&mut manager).unwrap();
    assert_eq!(next.start_lsn, Lsn(128));
    manager.flush().unwrap();

    assert_name_synced_before_write(&fs.log(), dir.path(), &segment2);
}

#[test]
fn a_torn_tail_is_salvaged_durably_before_the_first_append_cuts_it() {
    let dir = TempDir::new().unwrap();
    let mut first = WalManager::create(dir.path(), config()).unwrap();
    append(&mut first).unwrap();
    first.flush().unwrap();
    drop(first);
    // The first 20 bytes of a second record: a crash mid-write.
    let segment = segment_path(dir.path(), 1);
    let whole = std::fs::read(&segment).unwrap();
    let mut torn = whole.clone();
    torn.extend_from_slice(&whole[..20]);
    std::fs::write(&segment, &torn).unwrap();

    let fs = RecordingFs::default();
    let mut manager = WalManager::open_with_fs(dir.path(), config(), fs.clone()).unwrap();
    // Opening changes nothing: recovery may still fail after it.
    assert_eq!(std::fs::read(&segment).unwrap(), torn);
    assert!(
        !fs.log().iter().any(|op| matches!(op, Op::SetLen(..))),
        "{:#?}",
        fs.log()
    );

    let next = append(&mut manager).unwrap();
    assert_eq!(next.start_lsn, Lsn(64));
    manager.flush().unwrap();

    let salvage_dir = dir.path().join(WAL_SALVAGE_DIR);
    let salvage = salvage_dir.join(format!("{:020}-{:020}.torn", 1, 64));
    assert_eq!(std::fs::read(&salvage).unwrap(), whole[..20]);
    assert_eq!(manager.salvaged_tails(), std::slice::from_ref(&salvage));
    // The copy and its name are durable before the segment is cut, and the
    // segment is cut before the new record goes where the tail was.
    let log = fs.log();
    let copied = position(&log, &Op::Write(salvage.clone()));
    let copy_synced = position_after(&log, copied, &Op::SyncData(salvage.clone()));
    let name_synced = position_after(&log, copy_synced, &Op::SyncDir(salvage_dir.clone()));
    let cut = position(&log, &Op::SetLen(segment.clone(), 64));
    let appended = position(&log, &Op::Write(segment.clone()));
    assert!(name_synced < cut && cut < appended, "{log:#?}");
    drop(manager);

    let lsns: Vec<Lsn> = WalReader::new(dir.path(), config())
        .scan()
        .unwrap()
        .iter()
        .map(|record| record.lsn)
        .collect();
    assert_eq!(lsns, vec![Lsn(0), Lsn(64)]);
}

#[test]
fn a_salvage_copy_with_other_bytes_is_kept_beside_the_new_one() {
    let dir = TempDir::new().unwrap();
    let mut first = WalManager::create(dir.path(), config()).unwrap();
    append(&mut first).unwrap();
    first.flush().unwrap();
    drop(first);
    let segment = segment_path(dir.path(), 1);
    let whole = std::fs::read(&segment).unwrap();
    let mut torn = whole.clone();
    torn.extend_from_slice(&whole[..20]);
    std::fs::write(&segment, &torn).unwrap();
    // An earlier crash at the same position left a different tail.
    let salvage_dir = dir.path().join(WAL_SALVAGE_DIR);
    std::fs::create_dir_all(&salvage_dir).unwrap();
    let earlier = salvage_dir.join(format!("{:020}-{:020}.torn", 1, 64));
    std::fs::write(&earlier, b"earlier").unwrap();

    let mut manager = WalManager::open(dir.path(), config()).unwrap();
    append(&mut manager).unwrap();
    let copy = salvage_dir.join(format!("{:020}-{:020}.1.torn", 1, 64));
    assert_eq!(manager.salvaged_tails(), std::slice::from_ref(&copy));
    assert_eq!(std::fs::read(&earlier).unwrap(), b"earlier");
    assert_eq!(std::fs::read(&copy).unwrap(), whole[..20]);
}
