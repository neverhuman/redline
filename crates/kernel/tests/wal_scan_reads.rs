//! A WAL scan amortizes positioned reads using large windows.
//!
//! Bound the read count and preserve complete records across window
//! refills, including headers, bodies and oversized records.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use redlinedb_kernel::Result;
use redlinedb_kernel::engine::{CommitDurability, Engine, EngineConfig};
use redlinedb_kernel::format::{DEFAULT_PAGE_SIZE, Lsn, RelId, TxId};
use redlinedb_kernel::io::{FileHandle, FileSystem, StdFileHandle, StdFileSystem};
use redlinedb_kernel::txn::Isolation;
use redlinedb_kernel::wal::{WAL_HEADER_LEN, WalConfig, WalReader, WalRecord, WalRecordKind};
use tempfile::TempDir;

/// Rows written, one transaction each: several records apiece.
const ROWS: u64 = 2_000;

/// [`StdFileSystem`] that counts the reads made through its files.
#[derive(Clone, Default)]
struct CountingFs {
    reads: Rc<Cell<u64>>,
}

struct CountingFile {
    inner: StdFileHandle,
    reads: Rc<Cell<u64>>,
}

impl FileHandle for CountingFile {
    fn len(&self) -> Result<u64> {
        self.inner.len()
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.reads.set(self.reads.get() + 1);
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

fn config() -> EngineConfig {
    EngineConfig {
        rel_id: RelId(1),
        wal: WalConfig::default(),
        commit_durability: CommitDurability::Normal,
        lock_shards: 32,
        busy_timeout: Duration::from_millis(250),
        heap_lanes: 4,
        page_size: DEFAULT_PAGE_SIZE,
        buffer_pool_pages: 1_024,
        data_file_name: "data.redline".to_owned(),
    }
}

#[test]
fn scanning_the_wal_reads_each_segment_in_large_reads() {
    let temp = TempDir::new().expect("temp dir");
    {
        let engine = Engine::create(temp.path(), config()).expect("create");
        for row in 0..ROWS {
            let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
            engine
                .insert(&mut tx, format!("row {row:06}").into_bytes())
                .expect("insert");
            engine.commit(tx).expect("commit");
        }
    }
    let wal_dir = temp.path().join("wal");
    let wal_bytes: u64 = std::fs::read_dir(&wal_dir)
        .expect("wal dir")
        .map(|entry| entry.expect("entry").metadata().expect("metadata").len())
        .sum();

    let fs = CountingFs::default();
    let records = WalReader::new_with_fs(&wal_dir, config().wal, fs.clone())
        .scan()
        .expect("scan");
    assert!(
        records.len() as u64 >= 2 * ROWS,
        "{} records for {ROWS} committed inserts",
        records.len()
    );
    // One read per MiB of log, plus one per segment for a record that
    // crosses a window's end.
    let bound = wal_bytes.div_ceil(1 << 20) * 2 + 2;
    assert!(
        fs.reads.get() <= bound,
        "scanning {} records ({wal_bytes} bytes) made {} reads; at most {bound} expected",
        records.len(),
        fs.reads.get()
    );
}

/// Write valid linked records directly so their size and offset are exact.
fn assert_scanned_payloads(payloads: Vec<Vec<u8>>) {
    let temp = TempDir::new().expect("temp dir");
    let config = WalConfig {
        segment_bytes: 4 << 20,
        ..WalConfig::default()
    };
    let mut encoded = Vec::new();
    let mut expected = Vec::new();
    let mut prev_lsn = Lsn::ZERO;
    for payload in payloads {
        let record = WalRecord {
            lsn: Lsn(encoded.len() as u64),
            prev_lsn,
            tx_id: TxId(7),
            kind: WalRecordKind::PageDelta,
            payload,
        };
        encoded.extend(record.encode().expect("encode"));
        prev_lsn = record.lsn;
        expected.push(record);
    }
    let path = temp.path().join(format!("{:020}.wal", 1));
    std::fs::write(&path, &encoded).expect("write segment");
    let report = WalReader::new(temp.path(), config)
        .scan_report()
        .expect("scan");
    assert_eq!(report.records, expected);
    assert_eq!(report.valid_end_lsn, Lsn(encoded.len() as u64));
    assert!(!report.torn_tail);
    assert!(report.tail.is_none());
    assert_eq!(std::fs::read(path).expect("read segment"), encoded);
}

#[test]
fn scanning_preserves_a_record_larger_than_the_read_window() {
    let payload = (0..(1 << 20) + 137).map(|i| (i % 251) as u8).collect();
    assert_scanned_payloads(vec![payload, b"following record".to_vec()]);
}

#[test]
fn scanning_preserves_records_at_read_window_boundaries() {
    // The next header crosses the boundary, only its body crosses, or
    // its header starts exactly at the next window.
    for first_len in [(1 << 20) - 24, (1 << 20) - WAL_HEADER_LEN - 16, 1 << 20] {
        let first_payload = (0..first_len - WAL_HEADER_LEN)
            .map(|i| (i % 239) as u8)
            .collect();
        assert_scanned_payloads(vec![
            first_payload,
            (0..96).map(|i| (i + 1) as u8).collect(),
            b"last record".to_vec(),
        ]);
    }
}
