use std::path::Path;

use crate::format::{Lsn, TxId};
use crate::io::{FileHandle, FileSystem, StdFileSystem, create_dir_all_durable};
use crate::wal::{WAL_HEADER_LEN, WalRecord, WalRecordKind};
use crate::{Error, Result};

use super::salvage;
use super::*;

impl WalManager<StdFileSystem> {
    pub fn create(path: impl AsRef<Path>, config: WalConfig) -> Result<Self> {
        Self::create_with_fs(path, config, StdFileSystem)
    }

    pub fn open(path: impl AsRef<Path>, config: WalConfig) -> Result<Self> {
        Self::open_with_fs(path, config, StdFileSystem)
    }

    pub(crate) fn open_with_scan_summary(
        path: impl AsRef<Path>,
        config: WalConfig,
        summary: WalOpenScanSummary,
    ) -> Result<Self> {
        Self::open_with_fs_and_scan_summary(path, config, StdFileSystem, summary)
    }

    pub fn prune_segments_below_checkpoint_lsn(&mut self, checkpoint_lsn: Lsn) -> Result<usize> {
        let keep_segment = segment_for_lsn(checkpoint_lsn, self.config.segment_bytes);
        self.prune_segments_below(keep_segment)
    }

    pub fn prune_segments_below(&mut self, segment: u64) -> Result<usize> {
        if segment == 0 {
            return Ok(0);
        }

        // Lane E failpoint: armed before any WAL segment removal so harnesses
        // can crash between checkpoint completion and prune, observing whether
        // recovery still succeeds with pre-checkpoint segments on disk.
        crate::fail_point!("wal::prune");
        let mut removed = 0_usize;
        for candidate in self.segment_numbers()? {
            if candidate < segment && candidate < self.active_segment {
                let path = segment_path(&self.dir, candidate);
                if let Err(err) = std::fs::remove_file(path)
                    && err.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(err.into());
                }
                removed += 1;
            }
        }
        Ok(removed)
    }
}

impl<Fs: FileSystem> WalManager<Fs> {
    pub fn create_with_fs(path: impl AsRef<Path>, config: WalConfig, fs: Fs) -> Result<Self> {
        validate_config(&config)?;
        let dir = path.as_ref().to_path_buf();
        // A new WAL directory is a new name in the database root.
        create_dir_all_durable(&fs, &dir)?;
        let active_segment = 1;
        let active_offset = 0;
        let active_file = fs.open_rw_create(&segment_path(&dir, active_segment))?;
        // The first segment name has to survive power loss before any record
        // is treated as durable. Data fsync does not cover the directory entry.
        sync_wal_dir(&fs, &dir)?;
        Ok(Self {
            dir,
            fs,
            config,
            active_segment,
            active_offset,
            active_file,
            written_lsn: Lsn::ZERO,
            durable_lsn: Lsn::ZERO,
            prev_lsn: Lsn::ZERO,
            sync_counters: None,
            pending_tails: Vec::new(),
            salvaged: Vec::new(),
        })
    }

    pub fn open_with_fs(path: impl AsRef<Path>, config: WalConfig, fs: Fs) -> Result<Self> {
        validate_config(&config)?;
        let dir = path.as_ref().to_path_buf();
        create_dir_all_durable(&fs, &dir)?;
        let mut scan = WalReader::new_with_fs(&dir, config.clone(), fs);
        let report = scan.scan_report()?;
        let summary = report.open_summary();
        let fs = scan.into_fs();
        Self::open_prepared(dir, config, fs, summary)
    }

    pub(crate) fn open_with_fs_and_scan_summary(
        path: impl AsRef<Path>,
        config: WalConfig,
        fs: Fs,
        summary: WalOpenScanSummary,
    ) -> Result<Self> {
        validate_config(&config)?;
        let dir = path.as_ref().to_path_buf();
        create_dir_all_durable(&fs, &dir)?;
        Self::open_prepared(dir, config, fs, summary)
    }

    fn open_prepared(
        dir: std::path::PathBuf,
        config: WalConfig,
        fs: Fs,
        summary: WalOpenScanSummary,
    ) -> Result<Self> {
        let written_lsn = summary.valid_end_lsn;
        let prev_lsn = summary.last_record_lsn;
        let active_segment = segment_for_lsn(written_lsn, config.segment_bytes);
        let active_offset = offset_for_lsn(written_lsn, config.segment_bytes);
        let active_file = fs.open_rw_create(&segment_path(&dir, active_segment))?;
        // Appends go into this segment, and this open may just have created
        // it: an empty WAL, or a valid end exactly on a segment boundary. A
        // name left by a run that died before its directory sync may not be
        // durable either. Sync the directory before any record lands here.
        sync_wal_dir(&fs, &dir)?;
        // Bytes past the resume point are a torn tail. Recovery may still
        // fail after this open, so leave them in place: the writer copies
        // them to `wal/salvage/` and cuts them off before it writes.
        let pending_tails =
            salvage::pending_tails(&fs, &dir, active_segment, active_offset, active_file.len()?)?;

        Ok(Self {
            dir,
            fs,
            config,
            active_segment,
            active_offset,
            active_file,
            written_lsn,
            durable_lsn: Lsn::ZERO,
            prev_lsn,
            sync_counters: None,
            pending_tails,
            salvaged: Vec::new(),
        })
    }

    pub fn append(
        &mut self,
        kind: WalRecordKind,
        tx_id: TxId,
        payload: Vec<u8>,
    ) -> Result<WalAppend> {
        let encoded_len = WAL_HEADER_LEN
            .checked_add(payload.len())
            .ok_or(Error::CorruptWal("record length overflow"))?;
        self.settle_torn_tail()?;
        let append = self.reserve_append(encoded_len as u64)?;
        let record = WalRecord {
            lsn: append.start_lsn,
            prev_lsn: self.prev_lsn,
            tx_id,
            kind,
            payload,
        };
        let encoded = record.encode()?;
        self.write_encoded(append, &encoded)?;
        Ok(append)
    }

    pub fn flush(&mut self) -> Result<Lsn> {
        // Lane E failpoint: armed before the fsync that establishes WAL
        // durability so the harness can simulate "fsync skipped" or kernel
        // crash mid-fsync.
        //
        // The closure form lets the bench-matrix runner inject the
        // `return` action with meaningful semantics: it short-circuits
        // the fsync entirely and reports `Ok(written_lsn)` to the
        // caller, simulating a kernel that *claimed* the WAL was
        // durable while skipping the actual fsync(2). That is the
        // wal-fsync-skipped scenario; the strict gate must catch it
        // by detecting acked rows that are missing post-recovery.
        // `panic`/`abort` actions still kill the thread before
        // `sync_data` runs, exactly like the original site.
        let written = self.written_lsn;
        let _ = written;
        crate::fail_point!("wal::flush", |_| { Ok(written) });
        // Unlike `wal::flush`, this one fails the fsync: the written bytes
        // stay in the file, but the manager never calls them durable.
        crate::failpoints::io_error_under("wal::flush_error", &self.dir)?;
        self.active_file.sync_data()?;
        // Lane BH P1 #7: count fdatasync calls so the bench harness
        // can surface them on Redline rows. The bump only fires when
        // the manager is owned by a `WalCoordinator` (raw recovery
        // scans leave the counter `None`).
        if let Some(counters) = &self.sync_counters {
            counters.bump_fdatasync();
        }
        self.durable_lsn = self.written_lsn;
        Ok(self.durable_lsn)
    }

    pub fn written_lsn(&self) -> Lsn {
        self.written_lsn
    }

    pub fn durable_lsn(&self) -> Lsn {
        self.durable_lsn
    }

    pub fn wal_dir(&self) -> &Path {
        &self.dir
    }

    pub fn write_encoded(&mut self, append: WalAppend, encoded: &[u8]) -> Result<()> {
        self.write_encoded_batch(&[QueuedWalRecord {
            append,
            encoded: encoded.to_vec(),
        }])
    }

    /// Write a drained queue in as few positional writes as possible.
    /// A run stops at a segment boundary or a gap in LSN. Each run is one
    /// `write_all_at` and one `wal::write_encoded` failpoint. Record bytes
    /// are unchanged.
    pub(crate) fn write_encoded_batch(&mut self, records: &[QueuedWalRecord]) -> Result<()> {
        self.write_encoded_batch_staged(records)
            .map_err(|(_, err)| err)
    }

    /// [`Self::write_encoded_batch`], naming the step that failed: the
    /// segment rotation or the write itself.
    pub(crate) fn write_encoded_batch_staged(
        &mut self,
        records: &[QueuedWalRecord],
    ) -> std::result::Result<(), (WalFailureStage, Error)> {
        if !records.is_empty() {
            // A torn tail left by the last run goes to `wal/salvage/` before
            // any record, or a rotation, lands past it.
            self.settle_torn_tail()
                .map_err(|err| (WalFailureStage::Write, err))?;
        }
        let mut index = 0;
        while index < records.len() {
            self.prepare_for_record(records[index].append, records[index].encoded.len())?;
            let offset = self.active_offset;
            let mut bytes = Vec::new();
            let mut last_start = records[index].append.start_lsn;
            let mut last_end = records[index].append.end_lsn;
            loop {
                let record = &records[index];
                let at = offset + bytes.len() as u64;
                let expected = Lsn((self.active_segment - 1) * self.config.segment_bytes + at);
                if record.append.start_lsn != expected {
                    break;
                }
                if at > 0 && at + record.encoded.len() as u64 > self.config.segment_bytes {
                    break;
                }
                bytes.extend_from_slice(&record.encoded);
                last_start = record.append.start_lsn;
                last_end = record.append.end_lsn;
                index += 1;
                if index == records.len() {
                    break;
                }
            }
            if bytes.is_empty() {
                return Err((
                    WalFailureStage::Write,
                    Error::CorruptWal("record lsn does not match write position"),
                ));
            }
            self.emit_wal_bytes(offset, &bytes, last_end, last_start)
                .map_err(|err| (WalFailureStage::Write, err))?;
        }
        Ok(())
    }

    fn prepare_for_record(
        &mut self,
        append: WalAppend,
        encoded_len: usize,
    ) -> std::result::Result<(), (WalFailureStage, Error)> {
        let write_err = |err| (WalFailureStage::Write, err);
        let rotate_err = |err| (WalFailureStage::Rotate, err);
        if encoded_len > self.config.segment_bytes as usize {
            return Err(write_err(Error::CorruptWal(
                "record larger than wal segment",
            )));
        }
        // An exact fill leaves the cursor at `segment_bytes`. The next
        // reserved LSN is the following segment, and that LSN compares
        // equal to `(segment - 1) * size + offset` when offset == size,
        // so the mismatch check below would not rotate.
        if self.active_offset >= self.config.segment_bytes {
            self.rotate_segment().map_err(rotate_err)?;
        }
        let expected_lsn =
            Lsn((self.active_segment - 1) * self.config.segment_bytes + self.active_offset);
        if expected_lsn != append.start_lsn
            && self.active_offset > 0
            && self.active_offset + encoded_len as u64 > self.config.segment_bytes
        {
            self.rotate_segment().map_err(rotate_err)?;
        }
        let expected_lsn =
            Lsn((self.active_segment - 1) * self.config.segment_bytes + self.active_offset);
        if expected_lsn != append.start_lsn {
            return Err(write_err(Error::CorruptWal(
                "record lsn does not match write position",
            )));
        }
        Ok(())
    }

    fn emit_wal_bytes(
        &mut self,
        offset: u64,
        bytes: &[u8],
        end_lsn: Lsn,
        last_start_lsn: Lsn,
    ) -> Result<()> {
        // Lane E failpoint: armed before the bytes land, once per
        // physical write rather than once per queued record.
        crate::fail_point!("wal::write_encoded");
        crate::failpoints::io_error_under("wal::write_error", &self.dir)?;
        self.active_file.write_all_at(offset, bytes)?;
        // Lane BH P1 #7: count the pwrite-equivalent before bumping
        // the offset; the bench harness reads this through
        // `WalCoordinator::sync_counters_snapshot`.
        if let Some(counters) = &self.sync_counters {
            counters.bump_pwrite();
        }
        self.active_offset = offset + bytes.len() as u64;
        self.written_lsn = end_lsn;
        self.prev_lsn = last_start_lsn;
        Ok(())
    }

    fn rotate_segment(&mut self) -> Result<()> {
        self.active_file.sync_data()?;
        // Lane BH P1 #7: rotation also performs an fdatasync to
        // make the trailing block durable before swapping segment
        // files; counted alongside the commit-path fdatasync.
        if let Some(counters) = &self.sync_counters {
            counters.bump_fdatasync();
        }
        // The next segment's name has to be durable before any record goes
        // into it. Move to it only after the directory sync succeeds, so a
        // failure leaves the manager on the old segment and a retry creates
        // and syncs again.
        let next_segment = self.active_segment + 1;
        let next_file = self
            .fs
            .open_rw_create(&segment_path(&self.dir, next_segment))?;
        sync_wal_dir(&self.fs, &self.dir)?;
        self.active_segment = next_segment;
        self.active_offset = 0;
        self.active_file = next_file;
        Ok(())
    }

    fn reserve_append(&mut self, encoded_len: u64) -> Result<WalAppend> {
        if encoded_len > self.config.segment_bytes {
            return Err(Error::CorruptWal("record larger than wal segment"));
        }

        if self.active_offset > 0 && self.active_offset + encoded_len > self.config.segment_bytes {
            self.rotate_segment()?;
        }

        let lsn = Lsn((self.active_segment - 1) * self.config.segment_bytes + self.active_offset);
        let end_lsn = Lsn(lsn.0 + encoded_len);
        Ok(WalAppend {
            start_lsn: lsn,
            end_lsn,
        })
    }

    fn segment_numbers(&self) -> Result<Vec<u64>> {
        if !self.dir.exists() {
            return Ok(Vec::new());
        }

        let mut segments = Vec::new();
        for name in self.fs.read_dir_names(&self.dir)? {
            if let Some(segment) = parse_segment_name(&name) {
                segments.push(segment);
            }
        }
        segments.sort_unstable();
        Ok(segments)
    }
}

/// Fsync the WAL directory so a segment name created in it is durable.
///
/// The `wal::sync_dir` failpoint injects an I/O error here. `return` fails
/// every WAL directory sync. `return(<text>)` fails only directories whose
/// path contains `<text>` and syncs the others, so a test can confine the
/// fault to its own temp directory while other tests share the process.
fn sync_wal_dir<Fs: FileSystem>(fs: &Fs, dir: &Path) -> Result<()> {
    crate::fail_point!("wal::sync_dir", |only_under: Option<String>| {
        match only_under {
            Some(text) if !dir.to_string_lossy().contains(text.as_str()) => fs.sync_dir(dir),
            _ => Err(std::io::Error::other("wal::sync_dir failpoint").into()),
        }
    });
    fs.sync_dir(dir)
}

#[cfg(test)]
#[path = "write_tests.rs"]
mod tests;
