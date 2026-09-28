//! Workplan R9, steps 1 and 2: recovery checks that the WAL it replays is
//! whole before it changes any file.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use redlinedb_kernel::Error;
use redlinedb_kernel::engine::Engine;
use redlinedb_kernel::txn::Isolation;
use redlinedb_kernel::wal::WalReader;
use tempfile::TempDir;

use super::generations::{files_under, fill_until_segment, highest_segment, open_err, payload};
use super::{config, next_record_len};

fn segment_path(root: &Path, segment: u64) -> std::path::PathBuf {
    root.join("wal").join(format!("{segment:020}.wal"))
}

#[test]
fn corrupt_length_in_final_segment_fails_closed_without_truncating() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    for idx in 0..4_u64 {
        let mut tx = engine.begin(Isolation::Snapshot).unwrap();
        engine.insert(&mut tx, payload(idx)).unwrap();
        engine.commit(tx).unwrap();
    }
    drop(engine);

    // Give the second record a length that runs past the end of the file.
    // The scan used to read that as a torn tail, drop the committed records
    // after it, and truncate the segment there.
    let wal_path = segment_path(temp.path(), 1);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&wal_path)
        .unwrap();
    let file_len = file.metadata().unwrap().len();
    let second = next_record_len(&mut file, 0);
    let past_the_end = (file_len - second) as u32;
    file.seek(SeekFrom::Start(second + 12)).unwrap();
    file.write_all(&past_the_end.to_le_bytes()).unwrap();
    file.sync_data().unwrap();
    drop(file);
    let before = files_under(temp.path());

    let err = open_err(temp.path(), config());
    assert_eq!(err, Error::CorruptWal("valid wal record after torn tail"));
    assert_eq!(
        files_under(temp.path()),
        before,
        "a failed open changed files"
    );
}

#[test]
fn corrupt_length_past_the_segment_size_fails_closed() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    for idx in 0..3_u64 {
        let mut tx = engine.begin(Isolation::Snapshot).unwrap();
        engine.insert(&mut tx, payload(idx)).unwrap();
        engine.commit(tx).unwrap();
    }
    drop(engine);

    let wal_path = segment_path(temp.path(), 1);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&wal_path)
        .unwrap();
    let second = next_record_len(&mut file, 0);
    file.seek(SeekFrom::Start(second + 12)).unwrap();
    file.write_all(&u32::MAX.to_le_bytes()).unwrap();
    file.sync_data().unwrap();
    drop(file);
    let before = files_under(temp.path());

    let err = open_err(temp.path(), config());
    assert_eq!(err, Error::CorruptWal("valid wal record after torn tail"));
    assert_eq!(
        files_under(temp.path()),
        before,
        "a failed open changed files"
    );
}

#[test]
fn a_torn_final_record_still_opens() {
    // The tail a crash leaves, the start of a record and nothing after it,
    // is still discarded as before.
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let mut rows = Vec::new();
    for idx in 0..3_u64 {
        let mut tx = engine.begin(Isolation::Snapshot).unwrap();
        let row = engine.insert(&mut tx, payload(idx)).unwrap();
        engine.commit(tx).unwrap();
        rows.push((row, payload(idx)));
    }
    drop(engine);
    let wal_path = segment_path(temp.path(), 1);
    let mut bytes = Vec::new();
    OpenOptions::new()
        .read(true)
        .open(&wal_path)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    // Half of a copy of the first record, as if a write stopped midway.
    let mut file = OpenOptions::new().append(true).open(&wal_path).unwrap();
    file.write_all(&bytes[..700]).unwrap();
    drop(file);

    let reopened = Engine::open(temp.path(), config()).unwrap();
    super::generations::assert_rows_exact(&reopened, &rows);
}

#[test]
fn missing_first_wal_segment_without_checkpoint_fails_open() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let mut rows = Vec::new();
    fill_until_segment(&engine, temp.path(), 3, &mut rows);
    drop(engine);

    // No checkpoint ever ran, so recovery needs the WAL from LSN zero. The
    // remaining segments hold only a suffix of the committed rows.
    std::fs::remove_file(segment_path(temp.path(), 1)).unwrap();
    let before = files_under(temp.path());

    let err = open_err(temp.path(), config());
    assert_eq!(
        err,
        Error::CorruptWal(
            "wal does not start at lsn 0 and no checkpoint covers the records before it"
        )
    );
    assert_eq!(
        files_under(temp.path()),
        before,
        "a failed open changed files"
    );
}

#[test]
fn missing_middle_segment_fails_open() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let mut rows = Vec::new();
    fill_until_segment(&engine, temp.path(), 3, &mut rows);
    drop(engine);

    std::fs::remove_file(segment_path(temp.path(), 2)).unwrap();
    let before = files_under(temp.path());

    let err = open_err(temp.path(), config());
    assert_eq!(
        err,
        Error::CorruptWal("record prev_lsn does not match the previous record")
    );
    assert_eq!(
        files_under(temp.path()),
        before,
        "a failed open changed files"
    );
}

#[test]
fn missing_segment_before_a_torn_final_segment_fails_open() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let mut rows = Vec::new();
    fill_until_segment(&engine, temp.path(), 3, &mut rows);
    drop(engine);
    let last = highest_segment(temp.path());

    // The final segment holds only the torn start of a record, so no record
    // after the gap can show that the segment before it is missing.
    OpenOptions::new()
        .write(true)
        .open(segment_path(temp.path(), last))
        .unwrap()
        .set_len(5)
        .unwrap();
    std::fs::remove_file(segment_path(temp.path(), last - 1)).unwrap();
    let before = files_under(temp.path());

    let err = open_err(temp.path(), config());
    assert_eq!(
        err,
        Error::CorruptWal("wal segment missing between retained segments")
    );
    assert_eq!(
        files_under(temp.path()),
        before,
        "a failed open changed files"
    );
}

#[test]
fn wal_pruned_past_the_checkpoint_fails_open() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let mut rows = Vec::new();
    fill_until_segment(&engine, temp.path(), 2, &mut rows);
    let checkpoint = engine.checkpoint().unwrap();
    let segment_bytes = config().wal.segment_bytes;
    let checkpoint_segment = checkpoint.checkpoint_lsn.0 / segment_bytes + 1;
    // Records after the checkpoint still land in its segment, so the first
    // record left after the removal follows one past the checkpoint.
    assert!(checkpoint.checkpoint_lsn.0 % segment_bytes < segment_bytes / 2);
    fill_until_segment(&engine, temp.path(), checkpoint_segment + 2, &mut rows);
    drop(engine);

    // The segment holding the checkpoint's redo start is gone, while later
    // segments remain: the records between them are lost.
    for segment in 1..=checkpoint_segment {
        let path = segment_path(temp.path(), segment);
        if path.exists() {
            std::fs::remove_file(path).unwrap();
        }
    }
    let before = files_under(temp.path());

    let err = open_err(temp.path(), config());
    assert_eq!(
        err,
        Error::CorruptWal("wal starts after the checkpoint redo lsn")
    );
    assert_eq!(
        files_under(temp.path()),
        before,
        "a failed open changed files"
    );
}

#[test]
fn salvage_scan_reports_the_record_after_a_torn_tail() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    for idx in 0..3_u64 {
        let mut tx = engine.begin(Isolation::Snapshot).unwrap();
        engine.insert(&mut tx, payload(idx)).unwrap();
        engine.commit(tx).unwrap();
    }
    drop(engine);
    let wal_path = segment_path(temp.path(), 1);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&wal_path)
        .unwrap();
    let second = next_record_len(&mut file, 0);
    file.seek(SeekFrom::Start(second + 12)).unwrap();
    file.write_all(&u32::MAX.to_le_bytes()).unwrap();
    drop(file);

    let err = WalReader::new(temp.path().join("wal"), config().wal)
        .scan_report()
        .unwrap_err();
    assert_eq!(err, Error::CorruptWal("valid wal record after torn tail"));

    let report = WalReader::new(temp.path().join("wal"), config().wal)
        .salvage_after_torn_tail(true)
        .scan_report()
        .unwrap();
    let tail = report.tail.expect("a torn tail");
    assert_eq!((tail.segment, tail.offset), (1, second));
    assert!(
        tail.valid_record_after
            .is_some_and(|offset| offset > second)
    );
    assert_eq!(report.records.len(), 1);
}

#[test]
fn a_first_record_whose_prev_lsn_does_not_precede_it_fails_the_scan() {
    // The scan cannot check the first record's link against a predecessor,
    // but the log's first record names none, and any other names an
    // earlier one.
    let temp = TempDir::new().unwrap();
    let wal_dir = temp.path().join("wal");
    std::fs::create_dir_all(&wal_dir).unwrap();
    let record = redlinedb_kernel::wal::WalRecord {
        lsn: redlinedb_kernel::format::Lsn::ZERO,
        prev_lsn: redlinedb_kernel::format::Lsn(4096),
        tx_id: redlinedb_kernel::format::TxId(1),
        kind: redlinedb_kernel::wal::WalRecordKind::Commit,
        payload: Vec::new(),
    };
    std::fs::write(segment_path(temp.path(), 1), record.encode().unwrap()).unwrap();

    let err = WalReader::new(&wal_dir, config().wal)
        .scan_report()
        .unwrap_err();
    assert_eq!(
        err,
        Error::CorruptWal("record prev_lsn does not precede the record")
    );
}
