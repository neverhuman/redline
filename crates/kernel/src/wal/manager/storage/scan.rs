use std::path::Path;

use crate::format::Lsn;
use crate::format::bytes::{read_u32, read_u64};
use crate::io::{FileHandle, FileSystem, StdFileSystem};
use crate::wal::{WAL_HEADER_LEN, WAL_MAGIC, WalRecord};
use crate::{Error, Result};

use super::*;

/// How many record start positions the torn-tail probe reads at once.
const PROBE_CHUNK: u64 = 1 << 20;

/// How many bytes of a segment the scan reads at once.
const SCAN_WINDOW: u64 = 1 << 20;

/// The bytes of one segment, read [`SCAN_WINDOW`] at a time. A scan then
/// reads a segment in a few large reads, not two small ones per record.
#[derive(Default)]
struct SegmentWindow {
    start: u64,
    bytes: Vec<u8>,
}

impl SegmentWindow {
    /// The `len` bytes at `offset`. The caller has checked that they lie
    /// within the segment's `file_len`.
    fn read<F: FileHandle>(
        &mut self,
        file: &mut F,
        file_len: u64,
        offset: u64,
        len: usize,
    ) -> Result<&[u8]> {
        let held_end = self.start + self.bytes.len() as u64;
        if offset < self.start || offset + len as u64 > held_end {
            let want = (len as u64).max(SCAN_WINDOW).min(file_len - offset);
            self.bytes.resize(want as usize, 0);
            file.read_exact_at(offset, &mut self.bytes)?;
            self.start = offset;
        }
        let at = (offset - self.start) as usize;
        Ok(&self.bytes[at..at + len])
    }
}

impl WalReader<StdFileSystem> {
    pub fn new(path: impl AsRef<Path>, config: WalConfig) -> Self {
        Self::new_with_fs(path, config, StdFileSystem)
    }
}

impl<Fs: FileSystem> WalReader<Fs> {
    pub fn new_with_fs(path: impl AsRef<Path>, config: WalConfig, fs: Fs) -> Self {
        Self {
            dir: path.as_ref().to_path_buf(),
            fs,
            config,
            salvage_after_torn_tail: false,
        }
    }

    /// Accept a whole record after a torn tail and report where it is in
    /// [`TornTail::valid_record_after`], for inspecting a damaged log. A scan
    /// that does not salvage fails there, because the bytes before that
    /// record are damaged history, not the end of the log.
    pub fn salvage_after_torn_tail(mut self, salvage: bool) -> Self {
        self.salvage_after_torn_tail = salvage;
        self
    }

    pub fn scan(&mut self) -> Result<Vec<WalRecord>> {
        Ok(self.scan_report()?.records)
    }

    pub fn scan_report(&mut self) -> Result<WalScanReport> {
        validate_config(&self.config)?;
        let segment_numbers = self.segment_numbers()?;
        let mut records: Vec<WalRecord> = Vec::new();
        let mut segments = Vec::with_capacity(segment_numbers.len());
        let mut tail: Option<TornTail> = None;

        for (segment_index, segment) in segment_numbers.iter().enumerate() {
            let is_last_segment = segment_index + 1 == segment_numbers.len();
            let path = segment_path(&self.dir, *segment);
            // Recovery scans before it decides anything; a scan never writes.
            let mut file = self.fs.open_ro(&path)?;
            let file_len = file.len()?;
            segments.push(WalSegmentInfo {
                number: *segment,
                len: file_len,
            });
            let mut offset = 0_u64;
            let mut window = SegmentWindow::default();

            while offset < file_len {
                if tail.is_some() {
                    return Err(Error::CorruptWal("wal bytes after torn tail"));
                }

                let is_tail_candidate = is_last_segment;
                let remaining = file_len - offset;
                let torn = |reason| TornTail {
                    segment: *segment,
                    offset,
                    file_len,
                    reason,
                    valid_record_after: None,
                };
                if remaining < WAL_HEADER_LEN as u64 {
                    if is_tail_candidate {
                        tail = Some(torn(TornTailReason::PartialHeader));
                        break;
                    }
                    return Err(Error::CorruptWal("partial record header before final tail"));
                }

                let header = window.read(&mut file, file_len, offset, WAL_HEADER_LEN)?;
                let payload_len = read_u32(header, 12)? as u64;
                let record_len = match (WAL_HEADER_LEN as u64).checked_add(payload_len) {
                    Some(record_len) => record_len,
                    None if is_tail_candidate => {
                        tail = Some(torn(TornTailReason::LengthOverflow));
                        break;
                    }
                    None => return Err(Error::CorruptWal("record length overflow")),
                };

                if record_len > self.config.segment_bytes {
                    if is_tail_candidate {
                        tail = Some(torn(TornTailReason::LengthExceedsSegment));
                        break;
                    }
                    return Err(Error::CorruptWal("record length exceeds segment size"));
                }

                if remaining < record_len {
                    if is_tail_candidate {
                        tail = Some(torn(TornTailReason::PartialBody));
                        break;
                    }
                    return Err(Error::CorruptWal("partial record body before final tail"));
                }

                let encoded = window.read(&mut file, file_len, offset, record_len as usize)?;
                // Give ordinary records an owned decode buffer while retaining
                // batched file reads. The temporary stays through validation
                // and push; large records keep borrowing the read window.
                let ordinary = (record_len <= 512 << 10).then(|| encoded.to_vec());

                match WalRecord::decode(ordinary.as_deref().unwrap_or(encoded)) {
                    Ok(record) => {
                        validate_record_position(
                            &record,
                            *segment,
                            offset,
                            self.config.segment_bytes,
                        )?;
                        match records.last() {
                            Some(previous) if record.prev_lsn != previous.lsn => {
                                return Err(Error::CorruptWal(
                                    "record prev_lsn does not match the previous record",
                                ));
                            }
                            Some(_) => {}
                            // The first record's predecessor may be pruned,
                            // but it still has to come before the record.
                            None if !links_back(&record) => {
                                return Err(Error::CorruptWal(
                                    "record prev_lsn does not precede the record",
                                ));
                            }
                            None => {}
                        }
                        records.push(record);
                        offset += record_len;
                    }
                    Err(_) if is_tail_candidate && offset + record_len >= file_len => {
                        tail = Some(torn(TornTailReason::UndecodableRecord));
                        break;
                    }
                    Err(err) => return Err(err),
                }
            }

            if let Some(found) = tail.as_mut() {
                found.valid_record_after =
                    self.valid_record_after_tail(&mut file, found.segment, found.offset, file_len)?;
                if found.valid_record_after.is_some() && !self.salvage_after_torn_tail {
                    return Err(Error::CorruptWal("valid wal record after torn tail"));
                }
            }
        }

        let valid_end_lsn = records
            .last()
            .map(|record| Lsn(record.lsn.0 + record.encoded_len() as u64))
            .unwrap_or(Lsn::ZERO);
        let first_record = records.first().map(|record| WalRecordLink {
            lsn: record.lsn,
            prev_lsn: record.prev_lsn,
        });
        Ok(WalScanReport {
            records,
            valid_end_lsn,
            torn_tail: tail.is_some(),
            segments,
            first_record,
            tail,
        })
    }

    pub fn into_fs(self) -> Fs {
        self.fs
    }

    /// The offset of the first whole record after a torn tail that sits
    /// where its LSN says, if any.
    ///
    /// A crash mid-write tears the last record the writer appended, and
    /// nothing follows it. A whole record past the tear means the bytes at
    /// the tear were damaged inside the log, and the records after them
    /// were written, and may have been acknowledged. Every start position
    /// is tried, because the damaged length says nothing about where the
    /// next record begins. Requiring the record's LSN to equal its own
    /// position keeps a record-shaped run of bytes inside a payload from
    /// matching.
    fn valid_record_after_tail(
        &self,
        file: &mut Fs::File,
        segment: u64,
        tail_offset: u64,
        file_len: u64,
    ) -> Result<Option<u64>> {
        let header_len = WAL_HEADER_LEN as u64;
        let base = segment
            .checked_sub(1)
            .and_then(|index| index.checked_mul(self.config.segment_bytes))
            .ok_or_else(|| Error::CorruptWal("lsn overflow"))?;
        // The last position a whole header can start at.
        let Some(last_start) = file_len.checked_sub(header_len) else {
            return Ok(None);
        };
        let mut start = tail_offset + 1;
        while start <= last_start {
            let count = (last_start - start + 1).min(PROBE_CHUNK);
            let mut window = vec![0; (count + header_len - 1) as usize];
            file.read_exact_at(start, &mut window)?;
            for index in 0..count as usize {
                let header = &window[index..index + WAL_HEADER_LEN];
                if read_u32(header, 0)? != WAL_MAGIC {
                    continue;
                }
                let position = start + index as u64;
                if read_u64(header, 16)? != base + position {
                    continue;
                }
                let record_len = header_len + read_u32(header, 12)? as u64;
                if record_len > file_len - position {
                    continue;
                }
                let mut encoded = vec![0; record_len as usize];
                file.read_exact_at(position, &mut encoded)?;
                if WalRecord::decode(&encoded).is_ok() {
                    return Ok(Some(position));
                }
            }
            start += count;
        }
        Ok(None)
    }

    fn segment_numbers(&self) -> Result<Vec<u64>> {
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

/// A record's previous LSN names an earlier record, or zero for none.
fn links_back(record: &WalRecord) -> bool {
    if record.lsn == Lsn::ZERO {
        record.prev_lsn == Lsn::ZERO
    } else {
        record.prev_lsn < record.lsn
    }
}
