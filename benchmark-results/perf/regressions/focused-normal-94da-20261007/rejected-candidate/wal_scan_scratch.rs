//! Bound ordinary WAL scan scratch without restoring per-record I/O.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

use redlinedb_kernel::Result;
use redlinedb_kernel::format::{Lsn, TxId};
use redlinedb_kernel::io::{FileHandle, FileSystem, StdFileHandle, StdFileSystem};
use redlinedb_kernel::wal::{WAL_HEADER_LEN, WalConfig, WalReader, WalRecord, WalRecordKind};
use tempfile::TempDir;

/// Small records spanning several read windows.
const RECORDS: u64 = 2_200;

/// [`StdFileSystem`] that counts the reads made through its files.
#[derive(Clone, Default)]
struct CountingFs {
    reads: Rc<Cell<u64>>,
    largest_read: Rc<Cell<usize>>,
}

struct CountingFile {
    inner: StdFileHandle,
    reads: Rc<Cell<u64>>,
    largest_read: Rc<Cell<usize>>,
}

impl FileHandle for CountingFile {
    fn len(&self) -> Result<u64> {
        self.inner.len()
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.reads.set(self.reads.get() + 1);
        self.largest_read
            .set(self.largest_read.get().max(buf.len()));
        self.inner.read_exact_at(offset, buf)
    }

    fn write_all_at(&mut self, offset: u64, buf: &[u8]) -> Result<()> {
        self.inner.write_all_at(offset, buf)
    }

    fn sync_data(&self) -> Result<()> {
        self.inner.sync_data()
    }

    fn set_len(&self, len: u64) -> Result<()> {
        self.inner.set_len(len)
    }
}

impl CountingFs {
    fn wrap(&self, inner: StdFileHandle) -> CountingFile {
        CountingFile {
            inner,
            reads: Rc::clone(&self.reads),
            largest_read: Rc::clone(&self.largest_read),
        }
    }
}

impl FileSystem for CountingFs {
    type File = CountingFile;

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        StdFileSystem.create_dir_all(path)
    }

    fn read_dir_names(&self, path: &Path) -> Result<Vec<String>> {
        StdFileSystem.read_dir_names(path)
    }

    fn open_rw_create(&self, path: &Path) -> Result<Self::File> {
        Ok(self.wrap(StdFileSystem.open_rw_create(path)?))
    }

    fn open_rw_existing(&self, path: &Path) -> Result<Self::File> {
        Ok(self.wrap(StdFileSystem.open_rw_existing(path)?))
    }

    fn open_ro(&self, path: &Path) -> Result<Self::File> {
        Ok(self.wrap(StdFileSystem.open_ro(path)?))
    }

    fn sync_dir(&self, path: &Path) -> Result<()> {
        StdFileSystem.sync_dir(path)
    }
}

#[test]
fn ordinary_records_bound_scratch_without_losing_batched_reads() {
    let temp = TempDir::new().expect("scratch");
    let config = WalConfig {
        segment_bytes: 4 << 20,
        ..WalConfig::default()
    };
    let mut bytes = Vec::new();
    let mut expected = Vec::new();
    let mut previous = Lsn::ZERO;
    for i in 0..RECORDS {
        let record = WalRecord {
            lsn: Lsn(bytes.len() as u64),
            prev_lsn: previous,
            tx_id: TxId(7),
            kind: WalRecordKind::PageDelta,
            payload: vec![(i % 251) as u8; 1_024],
        };
        bytes.extend(record.encode().expect("encode"));
        previous = record.lsn;
        expected.push(record);
    }
    let path = temp.path().join(format!("{:020}.wal", 1));
    std::fs::write(&path, &bytes).expect("write WAL");
    let fs = CountingFs::default();
    let report = WalReader::new_with_fs(temp.path(), config, fs.clone())
        .scan_report()
        .expect("scan");
    assert_eq!(report.records, expected);
    assert_eq!(report.valid_end_lsn, Lsn(bytes.len() as u64));
    assert!(!report.torn_tail);
    assert!(
        fs.largest_read.get() <= 512 << 10,
        "ordinary records requested {} bytes of scratch",
        fs.largest_read.get()
    );
    // Retain the existing WAL scan read-count contract unchanged.
    let bound = (bytes.len() as u64).div_ceil(1 << 20) * 2 + 2;
    assert!(
        fs.reads.get() <= bound,
        "{} reads exceeds existing bound {bound}",
        fs.reads.get()
    );
    assert_eq!(std::fs::read(path).expect("read WAL"), bytes);
}

#[test]
fn ordinary_window_preserves_header_body_and_exact_boundary_records() {
    const WINDOW: usize = 512 << 10;
    for first_len in [WINDOW - 24, WINDOW - WAL_HEADER_LEN - 16, WINDOW] {
        let temp = TempDir::new().expect("scratch");
        let mut bytes = Vec::new();
        let mut expected = Vec::new();
        let mut previous = Lsn::ZERO;
        for size in [first_len - WAL_HEADER_LEN, 96, 37] {
            let record = WalRecord {
                lsn: Lsn(bytes.len() as u64),
                prev_lsn: previous,
                tx_id: TxId(7),
                kind: WalRecordKind::PageDelta,
                payload: (0..size).map(|i| (i % 239) as u8).collect(),
            };
            bytes.extend(record.encode().expect("encode"));
            previous = record.lsn;
            expected.push(record);
        }
        let path = temp.path().join(format!("{:020}.wal", 1));
        std::fs::write(&path, &bytes).expect("write WAL");
        let fs = CountingFs::default();
        let report = WalReader::new_with_fs(temp.path(), WalConfig::default(), fs.clone())
            .scan_report()
            .expect("scan");
        assert_eq!(report.records, expected);
        assert_eq!(report.valid_end_lsn, Lsn(bytes.len() as u64));
        assert!(report.tail.is_none());
        assert!(fs.largest_read.get() <= WINDOW);
        assert_eq!(std::fs::read(path).expect("read WAL"), bytes);
    }
}
