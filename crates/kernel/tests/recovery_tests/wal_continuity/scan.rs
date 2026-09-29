//! What a direct WAL scan reports for a torn tail and for a first record
//! whose `prev_lsn` does not precede it.

use super::*;

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
