//! Scanning the WAL reads each segment in a few large reads.
//!
//! Opening a database scans its whole WAL, and the scan read every record
//! with two small reads, one for the header and one for the body: 92,764
//! reads of about 150 bytes for the 14 MB log of a 20k-row database. It now
//! reads a segment a window at a time.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use redlinedb_kernel::Result;
use redlinedb_kernel::engine::{CommitDurability, Engine, EngineConfig};
use redlinedb_kernel::format::{DEFAULT_PAGE_SIZE, RelId};
use redlinedb_kernel::io::{FileHandle, FileSystem, StdFileHandle, StdFileSystem};
use redlinedb_kernel::txn::Isolation;
use redlinedb_kernel::wal::{WalConfig, WalReader};
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
