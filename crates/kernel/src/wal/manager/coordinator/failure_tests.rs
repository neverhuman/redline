//! Waiting on a WAL whose writer failed (workplan R7).
//!
//! The writer can make a record durable and then fail on a later write or
//! fsync before the waiter for the first record wakes. That waiter's record
//! is on disk, so it must get its LSN, not the later failure.

use std::io::ErrorKind;

use tempfile::TempDir;

use crate::Error;
use crate::format::{Lsn, TxId};
use crate::wal::{WalConfig, WalCoordinator, WalFailure, WalFailureStage, WalRecordKind};

fn config() -> WalConfig {
    WalConfig {
        segment_bytes: 1024 * 1024,
        group_commit_delay_us: 0,
        ..WalConfig::default()
    }
}

fn fail_writer(coord: &WalCoordinator, failure: WalFailure) {
    coord.shared.state.lock().unwrap().failure = Some(failure);
}

#[test]
fn flush_until_prefers_durable_over_failure() {
    let dir = TempDir::new().unwrap();
    let coord = WalCoordinator::create(dir.path(), config()).unwrap();
    let append = coord
        .append(WalRecordKind::Begin, TxId(1), b"begin".to_vec())
        .unwrap();
    let durable = coord.flush_until(append.end_lsn).unwrap();
    assert!(durable >= append.end_lsn);

    let failure = WalFailure {
        stage: WalFailureStage::Flush,
        kind: ErrorKind::Other,
        at_lsn: durable,
    };
    fail_writer(&coord, failure);

    assert_eq!(
        coord.flush_until(append.end_lsn),
        Ok(durable),
        "a durable record was reported failed"
    );
    assert_eq!(
        coord.write_until(append.end_lsn),
        Ok(durable),
        "a written record was reported failed"
    );

    // Past the durable LSN the failure decides, and says what failed.
    let beyond = Lsn(durable.0 + 1);
    let err = coord.flush_until(beyond).unwrap_err();
    assert_eq!(err, Error::from(failure));
    assert!(err.to_string().contains("wal writer failed"), "{err}");
    assert!(err.to_string().contains("flush"), "{err}");
    assert_eq!(coord.write_until(beyond).unwrap_err(), Error::from(failure));
}

#[test]
fn write_until_prefers_written_over_failure() {
    let dir = TempDir::new().unwrap();
    let coord = WalCoordinator::create(dir.path(), config()).unwrap();
    let append = coord
        .append(WalRecordKind::Begin, TxId(1), b"begin".to_vec())
        .unwrap();
    let written = coord.write_until(append.end_lsn).unwrap();
    assert!(written >= append.end_lsn);
    let durable = coord.durable_lsn().unwrap();
    assert!(durable < append.end_lsn, "nothing asked for an fsync");

    let failure = WalFailure {
        stage: WalFailureStage::Flush,
        kind: ErrorKind::Other,
        at_lsn: durable,
    };
    fail_writer(&coord, failure);

    assert_eq!(coord.write_until(append.end_lsn), Ok(written));
    // Written is not durable: a Strict waiter still sees the failure.
    assert_eq!(
        coord.flush_until(append.end_lsn).unwrap_err(),
        Error::from(failure)
    );
}
