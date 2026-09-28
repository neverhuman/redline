//! Workplan R9, steps 3 and 4: recovery keeps a torn WAL tail instead of
//! discarding it, changes no WAL byte until it has succeeded, and says what
//! it found.
//!
//! A crash mid-write leaves the start of a record at the end of the log.
//! Recovery does not replay it. Once recovery has succeeded, the bytes are
//! copied to `wal/salvage/<segment>-<offset>.torn` and only then cut from
//! the segment, so a failed open leaves the WAL exactly as it was and a
//! successful one keeps the evidence.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use redlinedb_kernel::engine::Engine;
use redlinedb_kernel::format::{Lsn, RowId, TxId};
use redlinedb_kernel::txn::Isolation;
use redlinedb_kernel::wal::{TornTailReason, WalReader, WalRecord, WalRecordKind};
use tempfile::TempDir;

use super::generations::{files_under, payload};
use super::{config, truncate_wal_tail};

fn wal_dir(root: &Path) -> PathBuf {
    root.join("wal")
}

fn segment_path(root: &Path, segment: u64) -> PathBuf {
    wal_dir(root).join(format!("{segment:020}.wal"))
}

fn salvage_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let dir = wal_dir(root).join("salvage");
    if !dir.exists() {
        return BTreeMap::new();
    }
    files_under(&dir)
}

fn commit_row(engine: &Engine, tag: u64) -> RowId {
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    let row = engine.insert(&mut tx, payload(tag)).unwrap();
    engine.commit(tx).unwrap();
    row
}

/// One committed row, then an uncommitted insert whose last record loses
/// its final three bytes, as a crash in the middle of that write leaves it.
fn torn_image(root: &Path) -> RowId {
    let engine = Engine::create(root, config()).unwrap();
    let row = commit_row(&engine, 1);
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    engine.insert(&mut tx, payload(2)).unwrap();
    drop(tx);
    drop(engine);
    truncate_wal_tail(root, 3);
    row
}

#[test]
fn torn_tail_is_preserved_in_salvage_file() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let row = torn_image(root);
    let scan = WalReader::new(wal_dir(root), config().wal)
        .scan_report()
        .unwrap();
    let tail = scan.tail.expect("the scan finds the torn record");
    let segment = std::fs::read(segment_path(root, tail.segment)).unwrap();
    let torn = segment[tail.offset as usize..].to_vec();
    assert!(!torn.is_empty());

    let reopened = Engine::open(root, config()).unwrap();

    // The torn bytes are kept, named after where they were, and cut from
    // the segment so the log ends at its last whole record.
    let salvage = salvage_files(root);
    let name = PathBuf::from(format!("{:020}-{:020}.torn", tail.segment, tail.offset));
    assert_eq!(
        salvage.keys().collect::<Vec<_>>(),
        vec![&name],
        "salvage directory holds {:?}",
        salvage.keys().collect::<Vec<_>>()
    );
    assert_eq!(salvage[&name], torn);
    assert_eq!(
        std::fs::metadata(segment_path(root, tail.segment))
            .unwrap()
            .len(),
        tail.offset
    );
    let mut tx = reopened.begin(Isolation::Snapshot).unwrap();
    assert_eq!(reopened.get(&mut tx, row).unwrap(), Some(payload(1)));
    drop(tx);
    // A tail the crash left and recovery cut off is not damage to the
    // database (the documented tail policy).
    assert_eq!(reopened.integrity_check().unwrap(), Vec::<String>::new());
    drop(reopened);

    let rescan = WalReader::new(wal_dir(root), config().wal)
        .scan_report()
        .unwrap();
    assert_eq!(rescan.tail, None);
    assert_eq!(rescan.valid_end_lsn, scan.valid_end_lsn);
    let again = Engine::open(root, config()).unwrap();
    let mut tx = again.begin(Isolation::Snapshot).unwrap();
    assert_eq!(again.get(&mut tx, row).unwrap(), Some(payload(1)));
}

#[test]
fn failed_recovery_leaves_wal_bytes_unchanged() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let engine = Engine::create(root, config()).unwrap();
    commit_row(&engine, 1);
    drop(engine);

    // A record that passes its checksum but whose payload does not decode:
    // the scan accepts it, and recovery fails on it only after it has
    // opened the WAL. Behind it, the torn start of one more record.
    let scan = WalReader::new(wal_dir(root), config().wal)
        .scan_report()
        .unwrap();
    let last = scan.records.last().unwrap();
    let broken = WalRecord {
        lsn: scan.valid_end_lsn,
        prev_lsn: last.lsn,
        tx_id: TxId(last.tx_id.0 + 1),
        kind: WalRecordKind::PageDelta,
        payload: vec![0xff; 8],
    }
    .encode()
    .unwrap();
    let next = WalRecord {
        lsn: Lsn(scan.valid_end_lsn.0 + broken.len() as u64),
        prev_lsn: scan.valid_end_lsn,
        tx_id: TxId(last.tx_id.0 + 1),
        kind: WalRecordKind::Commit,
        payload: vec![0; 17],
    }
    .encode()
    .unwrap();
    let mut bytes = std::fs::read(segment_path(root, 1)).unwrap();
    bytes.extend_from_slice(&broken);
    bytes.extend_from_slice(&next[..20]);
    std::fs::write(segment_path(root, 1), &bytes).unwrap();
    let before = files_under(&wal_dir(root));

    assert!(Engine::open(root, config()).is_err(), "the open succeeded");
    assert_eq!(
        files_under(&wal_dir(root)),
        before,
        "a failed recovery changed the WAL"
    );
}

#[test]
fn integrity_check_reports_a_torn_tail_inside_the_written_wal() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let engine = Engine::create(root, config()).unwrap();
    commit_row(&engine, 1);
    commit_row(&engine, 2);
    assert_eq!(engine.integrity_check().unwrap(), Vec::<String>::new());

    // The writer wrote and synced every record; cutting the last one short
    // under the running engine is damage, not a write still in flight.
    truncate_wal_tail(root, 3);

    let errors = engine.integrity_check().unwrap();
    assert!(
        errors.iter().any(|error| error.contains("torn tail")),
        "integrity_check returned {errors:?}"
    );
}

#[test]
fn a_torn_tail_in_the_next_segment_is_preserved_and_emptied() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let engine = Engine::create(root, config()).unwrap();
    let row = commit_row(&engine, 1);
    drop(engine);

    // The next record did not fit, so the writer rotated and died writing
    // it: the last whole record ends mid segment 1, and segment 2 holds
    // only the start of the record that follows it.
    let scan = WalReader::new(wal_dir(root), config().wal)
        .scan_report()
        .unwrap();
    let segment_bytes = config().wal.segment_bytes;
    let next = WalRecord {
        lsn: Lsn(segment_bytes),
        prev_lsn: scan.records.last().unwrap().lsn,
        tx_id: TxId(99),
        kind: WalRecordKind::PageDelta,
        payload: vec![5; 4000],
    }
    .encode()
    .unwrap();
    let torn = next[..3000].to_vec();
    std::fs::write(segment_path(root, 2), &torn).unwrap();
    let tail = WalReader::new(wal_dir(root), config().wal)
        .scan_report()
        .unwrap()
        .tail
        .expect("segment 2 reads as a torn tail");
    assert_eq!((tail.segment, tail.offset), (2, 0));
    assert_eq!(tail.reason, TornTailReason::PartialBody);

    let reopened = Engine::open(root, config()).unwrap();
    let salvage = salvage_files(root);
    let name = PathBuf::from(format!("{:020}-{:020}.torn", 2, 0));
    assert_eq!(salvage.get(&name), Some(&torn), "{:?}", salvage.keys());
    assert_eq!(std::fs::metadata(segment_path(root, 2)).unwrap().len(), 0);
    let later = commit_row(&reopened, 3);
    drop(reopened);

    let again = Engine::open(root, config()).unwrap();
    let mut tx = again.begin(Isolation::Snapshot).unwrap();
    assert_eq!(again.get(&mut tx, row).unwrap(), Some(payload(1)));
    assert_eq!(again.get(&mut tx, later).unwrap(), Some(payload(3)));
}

#[cfg(unix)]
#[test]
fn wal_scan_opens_segments_read_only() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let engine = Engine::create(root, config()).unwrap();
    commit_row(&engine, 1);
    drop(engine);
    let segment = segment_path(root, 1);
    std::fs::set_permissions(&segment, std::fs::Permissions::from_mode(0o444)).unwrap();

    let scan = WalReader::new(wal_dir(root), config().wal).scan_report();
    std::fs::set_permissions(&segment, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(scan.unwrap().records.len() > 1);
}

#[test]
fn recovery_report_names_the_torn_tail_and_its_salvage_file() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    torn_image(root);
    let tail = WalReader::new(wal_dir(root), config().wal)
        .scan_report()
        .unwrap()
        .tail
        .expect("a torn tail");

    let (engine, returned) = Engine::open_with_recovery_report(root, config()).unwrap();
    let report = engine
        .last_recovery_report()
        .expect("the engine keeps its report");
    assert_eq!(report, returned);
    assert!(report.torn_tail);
    let recovered = report.tail.expect("the report names the tail");
    assert_eq!(
        (recovered.segment, recovered.offset),
        (tail.segment, tail.offset)
    );
    assert_eq!(recovered.bytes, tail.file_len - tail.offset);
    assert_eq!(recovered.reason, tail.reason);
    assert_eq!(
        recovered.lsn.0,
        (tail.segment - 1) * config().wal.segment_bytes + tail.offset
    );
    assert_eq!(
        recovered.salvage,
        vec![
            wal_dir(root)
                .join("salvage")
                .join(format!("{:020}-{:020}.torn", tail.segment, tail.offset))
        ]
    );
    drop(engine);

    // A database this process created has no report.
    let fresh = TempDir::new().unwrap();
    let created = Engine::create(fresh.path(), config()).unwrap();
    assert_eq!(created.last_recovery_report(), None);
}
