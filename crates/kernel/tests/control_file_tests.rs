//! Control file generations carry monotonic redo LSNs (workplan R5).

use redlinedb_kernel::Error;
use redlinedb_kernel::format::Lsn;
use redlinedb_kernel::storage::{CONTROL_LEN, CONTROL_VERSION, ControlFile, ControlStore};
use tempfile::TempDir;

#[test]
fn control_store_refuses_a_redo_lsn_below_the_previous_generation() {
    let temp = TempDir::new().unwrap();
    let store = ControlStore::new(temp.path()).unwrap();
    let first = store.write_next(None, Lsn(100), Lsn(150), 3).unwrap();

    // The first generation may already have pruned the WAL below its LSNs,
    // so a later one that records less would send recovery into WAL that is
    // gone.
    for (checkpoint_lsn, heap_redo_lsn) in [(Lsn(99), Lsn(150)), (Lsn(100), Lsn(149))] {
        assert_eq!(
            store.write_next(Some(first), checkpoint_lsn, heap_redo_lsn, 3),
            Err(Error::CorruptPage(
                "checkpoint redo lsn is below the previous generation's"
            ))
        );
    }
    assert_eq!(store.load_latest().unwrap(), Some(first));

    let second = store
        .write_next(Some(first), Lsn(100), Lsn(150), 4)
        .unwrap();
    assert_eq!(second.generation, 2);
    assert_eq!(store.load_latest().unwrap(), Some(second));
}

#[test]
fn control_store_refuses_a_heap_redo_lsn_below_the_checkpoint_lsn() {
    let temp = TempDir::new().unwrap();
    let store = ControlStore::new(temp.path()).unwrap();
    assert_eq!(
        store.write_next(None, Lsn(100), Lsn(99), 3),
        Err(Error::CorruptPage(
            "checkpoint heap redo lsn is below its checkpoint lsn"
        ))
    );
    assert_eq!(store.load_latest().unwrap(), None);
}

#[test]
fn version_one_control_file_starts_heap_redo_at_its_checkpoint_lsn() {
    // A file written before the heap redo LSN existed: version 1, bytes 40
    // and up zero.
    let control = ControlFile {
        generation: 4,
        checkpoint_lsn: Lsn(1234),
        page_count: 9,
        heap_redo_lsn: Lsn(1234),
    };
    let mut bytes = control.encode().unwrap();
    assert_eq!(CONTROL_VERSION, 2);
    assert_eq!(bytes.len(), CONTROL_LEN);
    bytes[4..8].copy_from_slice(&1_u32.to_le_bytes());
    bytes[40..48].fill(0);
    reseal(&mut bytes);
    assert_eq!(ControlFile::decode(&bytes).unwrap(), control);
}

#[test]
fn control_file_with_heap_redo_below_its_checkpoint_is_corrupt() {
    let control = ControlFile {
        generation: 4,
        checkpoint_lsn: Lsn(1234),
        page_count: 9,
        heap_redo_lsn: Lsn(1234),
    };
    let mut bytes = control.encode().unwrap();
    bytes[40..48].copy_from_slice(&1000_u64.to_le_bytes());
    reseal(&mut bytes);
    assert_eq!(
        ControlFile::decode(&bytes),
        Err(Error::CorruptPage(
            "control file heap redo lsn is below its checkpoint lsn"
        ))
    );
}

/// Recompute the checksum at bytes 8..12 over the file with that field
/// zeroed, as the control file format does.
fn reseal(bytes: &mut [u8]) {
    bytes[8..12].fill(0);
    let checksum = crc32fast::hash(bytes);
    bytes[8..12].copy_from_slice(&checksum.to_le_bytes());
}
