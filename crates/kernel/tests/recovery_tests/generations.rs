//! Workplan R6: every generation a control slot can fall back to keeps the
//! WAL and transaction status it needs, and damaged control or catalog
//! state fails the open instead of recovering an older or empty database.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use redlinedb_kernel::Error;
use redlinedb_kernel::engine::{Engine, EngineConfig};
use redlinedb_kernel::format::{PageId, RelId, RowId};
use redlinedb_kernel::txn::Isolation;
use tempfile::TempDir;

use super::{config, wal_segment_count};

/// A payload that says which row it belongs to, large enough that a few
/// dozen commits fill a 64 KiB segment.
pub(super) fn payload(tag: u64) -> Vec<u8> {
    let mut bytes = vec![(tag % 251) as u8; 1500];
    bytes[..8].copy_from_slice(&tag.to_le_bytes());
    bytes
}

/// The error an open fails with. An open that succeeds fails the test
/// without printing the whole engine.
pub(super) fn open_err(root: &Path, config: EngineConfig) -> Error {
    match Engine::open(root, config) {
        Ok(_) => panic!("the open succeeded"),
        Err(err) => err,
    }
}

/// Commit one row per transaction until the WAL reaches `segment`.
pub(super) fn fill_until_segment(
    engine: &Engine,
    root: &Path,
    segment: u64,
    rows: &mut Vec<(RowId, Vec<u8>)>,
) {
    let wal = root.join("wal");
    while wal_segment_count(&wal).last().copied().unwrap_or(0) < segment {
        let bytes = payload(rows.len() as u64);
        let mut tx = engine.begin(Isolation::Snapshot).unwrap();
        let row = engine.insert(&mut tx, bytes.clone()).unwrap();
        engine.commit(tx).unwrap();
        rows.push((row, bytes));
    }
}

pub(super) fn highest_segment(root: &Path) -> u64 {
    wal_segment_count(&root.join("wal"))
        .last()
        .copied()
        .unwrap_or(0)
}

/// Every file under `root` with its bytes.
pub(super) fn files_under(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                files.insert(path.strip_prefix(root).unwrap().to_path_buf(), bytes);
            }
        }
    }
    files
}

/// Assert every row reads back exactly and a scan of every heap page finds
/// each committed row once and nothing else.
pub(super) fn assert_rows_exact(engine: &Engine, rows: &[(RowId, Vec<u8>)]) {
    assert_rows_read_back(engine, rows);
    assert_scan_finds_each_row_once(engine, rows);
}

/// Assert every row reads back exactly by its row id.
pub(super) fn assert_rows_read_back(engine: &Engine, rows: &[(RowId, Vec<u8>)]) {
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    for (row, bytes) in rows {
        assert_eq!(
            engine.get(&mut tx, *row).unwrap().as_ref(),
            Some(bytes),
            "row {row:?} did not read back exactly"
        );
    }
}

/// How many times a scan of every heap page returns each (row, payload).
pub(super) fn scan_counts(engine: &Engine) -> BTreeMap<(RowId, Vec<u8>), usize> {
    let tx = engine.begin(Isolation::Snapshot).unwrap();
    let pages = engine
        .relation_entries(RelId(1))
        .unwrap()
        .iter()
        .map(|(_, ptr)| ptr.page_id.0)
        .fold(engine.heap_page_count().unwrap(), u64::max);
    let mut found = BTreeMap::new();
    for row in engine
        .parallel_scan_page_range(
            tx.snapshot(),
            Some(tx.id()),
            PageId(1)..PageId(pages + 1),
            Some(RelId(1)),
            1,
            None,
        )
        .unwrap()
    {
        *found.entry((row.row_id, row.payload)).or_insert(0_usize) += 1;
    }
    found
}

/// Assert a scan of every heap page finds every row, at least once, and
/// nothing else. For paths with the open duplicate-row defect, where a
/// scan may return a row twice but must not lose or invent one.
pub(super) fn assert_scan_finds_every_row(engine: &Engine, rows: &[(RowId, Vec<u8>)]) {
    let found = scan_counts(engine);
    let expected: std::collections::BTreeSet<_> = rows.iter().cloned().collect();
    let missing: Vec<u64> = expected
        .iter()
        .filter(|key| !found.contains_key(*key))
        .map(|(row, _)| row.0)
        .collect();
    let unexpected: Vec<u64> = found
        .keys()
        .filter(|key| !expected.contains(*key))
        .map(|(row, _)| row.0)
        .collect();
    assert!(
        missing.is_empty() && unexpected.is_empty(),
        "a page scan lost rows {missing:?} or returned rows it should not have {unexpected:?}"
    );
}

/// Assert a scan of every heap page finds each row once and nothing else.
pub(super) fn assert_scan_finds_each_row_once(engine: &Engine, rows: &[(RowId, Vec<u8>)]) {
    let found = scan_counts(engine);
    let expected: BTreeMap<_, _> = rows
        .iter()
        .map(|(row, bytes)| ((*row, bytes.clone()), 1_usize))
        .collect();
    if found != expected {
        // Name rows by id and payload tag, not by their bytes.
        let tag = |bytes: &Vec<u8>| u64::from_le_bytes(bytes[..8].try_into().unwrap());
        let describe = |map: &BTreeMap<(RowId, Vec<u8>), usize>| {
            map.iter()
                .map(|((row, bytes), count)| (row.0, tag(bytes), *count))
                .collect::<Vec<_>>()
        };
        let extra: BTreeMap<_, _> = found
            .iter()
            .filter(|(key, count)| expected.get(*key) != Some(*count))
            .map(|(key, count)| (key.clone(), *count))
            .collect();
        let missing: BTreeMap<_, _> = expected
            .iter()
            .filter(|(key, _)| !found.contains_key(*key))
            .map(|(key, count)| (key.clone(), *count))
            .collect();
        panic!(
            "a page scan found other rows or copies: (row, tag, copies) unexpected {:?}, missing {:?}",
            describe(&extra),
            describe(&missing)
        );
    }
}

pub(super) fn corrupt_byte(path: &Path, offset: u64) {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(&[0xdd]).unwrap();
    file.sync_data().unwrap();
}

/// A closed database with two checkpoints over a WAL of several segments
/// and commits after the second: its rows and both control files.
pub(super) struct TwoGenerations {
    pub(super) temp: TempDir,
    pub(super) config: EngineConfig,
    pub(super) rows: Vec<(RowId, Vec<u8>)>,
    pub(super) first: redlinedb_kernel::storage::ControlFile,
    pub(super) second: redlinedb_kernel::storage::ControlFile,
}

/// Generation 1 starts past segment 2 and generation 2 at least two
/// segments later, so the second checkpoint prunes WAL generation 1 does
/// not need, and would have pruned WAL it does need had it pruned below
/// its own checkpoint LSN.
pub(super) fn two_generations_over_pruned_segments() -> TwoGenerations {
    let config = config();
    assert_eq!(config.wal.segment_bytes, 65_536);
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config.clone()).unwrap();
    let mut rows = Vec::new();

    fill_until_segment(&engine, temp.path(), 3, &mut rows);
    let first = engine.checkpoint().unwrap();
    let first_segment = first.checkpoint_lsn.0 / config.wal.segment_bytes + 1;
    assert!(
        first_segment >= 3,
        "generation 1 should start past segment 2"
    );

    fill_until_segment(&engine, temp.path(), first_segment + 2, &mut rows);
    let second = engine.checkpoint().unwrap();
    assert!(
        second.checkpoint_lsn.0 / config.wal.segment_bytes + 1 >= first_segment + 2,
        "generation 2 should start at least two segments past generation 1"
    );

    for _ in 0..5 {
        let bytes = payload(rows.len() as u64);
        let mut tx = engine.begin(Isolation::Snapshot).unwrap();
        let row = engine.insert(&mut tx, bytes.clone()).unwrap();
        engine.commit(tx).unwrap();
        rows.push((row, bytes));
    }
    drop(engine);
    assert_eq!((first.generation, second.generation), (1, 2));
    TwoGenerations {
        temp,
        config,
        rows,
        first,
        second,
    }
}

#[test]
fn fallback_control_keeps_wal_needed_by_previous_generation() {
    let image = two_generations_over_pruned_segments();
    let root = image.temp.path();

    // Generation 2 lives in CONTROL_B. Damage it the way a torn write of
    // that slot would: its checksum no longer matches.
    corrupt_byte(&root.join("CONTROL_B"), 24);

    let (reopened, report) = Engine::open_with_recovery_report(root, image.config.clone()).unwrap();
    assert_eq!(reopened.checkpoint_info().unwrap(), Some(image.first));
    assert_eq!(report.replay_from_lsn, image.first.checkpoint_lsn);
    // The slot's generation cannot be read, so none is named as skipped.
    assert_eq!(report.skipped_generation, None);
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("CONTROL_B")),
        "the report does not say CONTROL_B was skipped: {:?}",
        report.warnings
    );
    // Every row reads back, and a page scan finds every row and nothing
    // else. It may find a row twice: generation 2 wrote its heap pages
    // before its control file, and with that control unreadable, heap redo
    // from generation 1 adds a second copy of each row generation 2 wrote
    // to a page generation 1 already had. That happens the same way when a
    // checkpoint dies between its page writes and its control write, with
    // no corrupt slot at all; it is a known defect in docs/launch/v5.0.0-evidence.md and
    // tracked by `fallback_after_corrupt_newer_control_scans_each_row_once`.
    assert_rows_read_back(&reopened, &image.rows);
    assert_scan_finds_every_row(&reopened, &image.rows);
    drop(reopened);

    // The fallback generation keeps working across another reopen.
    let reopened = Engine::open(root, image.config.clone()).unwrap();
    assert_rows_read_back(&reopened, &image.rows);
    assert_scan_finds_every_row(&reopened, &image.rows);
}

#[test]
fn a_valid_newer_generation_without_its_tx_status_falls_back() {
    let image = two_generations_over_pruned_segments();
    let root = image.temp.path();
    std::fs::remove_file(root.join(format!("TX_STATUS_{:020}", image.second.generation))).unwrap();

    let (reopened, report) = Engine::open_with_recovery_report(root, image.config.clone()).unwrap();
    assert_eq!(reopened.checkpoint_info().unwrap(), Some(image.first));
    assert_eq!(report.skipped_generation, Some(image.second.generation));
    assert!(!report.warnings.is_empty());
    assert_rows_exact(&reopened, &image.rows);
}

#[test]
fn corrupt_only_control_after_prune_fails_closed() {
    let image = two_generations_over_pruned_segments();
    let root = image.temp.path();
    // Generation 1's WAL prefix is gone, so without a control file the
    // WAL cannot say what the page file holds. One slot corrupt, the other
    // missing, used to read as "never checkpointed" and replay from zero.
    corrupt_byte(&root.join("CONTROL_B"), 24);
    std::fs::remove_file(root.join("CONTROL_A")).unwrap();
    assert!(
        wal_segment_count(&root.join("wal"))
            .first()
            .is_some_and(|segment| *segment > 1),
        "the test needs a pruned WAL"
    );
    let before = files_under(root);

    let err = open_err(root, image.config.clone());
    assert_eq!(
        err,
        Error::CorruptWal("no valid control file and the wal does not start at lsn 0")
    );
    assert_eq!(files_under(root), before, "a failed open changed files");
}

#[test]
fn corrupt_only_control_with_the_whole_wal_replays_it() {
    // A torn write of the first control file: the WAL was never pruned, so
    // recovery can still replay it from the start.
    let config = config();
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config.clone()).unwrap();
    let mut rows = Vec::new();
    fill_until_segment(&engine, temp.path(), 2, &mut rows);
    let first = engine.checkpoint().unwrap();
    assert_eq!(first.generation, 1);
    for _ in 0..3 {
        let bytes = payload(rows.len() as u64);
        let mut tx = engine.begin(Isolation::Snapshot).unwrap();
        let row = engine.insert(&mut tx, bytes.clone()).unwrap();
        engine.commit(tx).unwrap();
        rows.push((row, bytes));
    }
    drop(engine);
    corrupt_byte(&temp.path().join("CONTROL_A"), 24);

    let (reopened, report) = Engine::open_with_recovery_report(temp.path(), config).unwrap();
    assert_eq!(report.replay_from_lsn, redlinedb_kernel::format::Lsn::ZERO);
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("CONTROL_A")),
        "{:?}",
        report.warnings
    );
    assert_rows_exact(&reopened, &rows);
}

#[test]
fn checkpoint_keeps_only_two_tx_status_generations() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    for idx in 0..4_u64 {
        let mut tx = engine.begin(Isolation::Snapshot).unwrap();
        engine.insert(&mut tx, payload(idx)).unwrap();
        engine.commit(tx).unwrap();
        engine.checkpoint().unwrap();
    }
    drop(engine);
    let mut status_files: Vec<String> = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("TX_STATUS_"))
        .collect();
    status_files.sort();
    assert_eq!(
        status_files,
        vec![
            format!("TX_STATUS_{:020}", 3),
            format!("TX_STATUS_{:020}", 4)
        ]
    );
}

#[test]
fn corrupt_catalog_with_checkpoint_fails_closed() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    engine.insert(&mut tx, payload(1)).unwrap();
    engine.commit(tx).unwrap();
    engine.checkpoint().unwrap();
    drop(engine);

    // An undecodable schema file used to fall back to the empty bootstrap
    // schema, which drops every table the database had.
    let schema = temp.path().join("schema.redline");
    let len = std::fs::metadata(&schema).unwrap().len();
    corrupt_byte(&schema, len / 2);
    let before = files_under(temp.path());

    let err = open_err(temp.path(), config());
    assert_eq!(
        err,
        Error::CorruptPage("catalog file is unreadable and the wal holds no catalog snapshot")
    );
    assert_eq!(
        files_under(temp.path()),
        before,
        "a failed open changed files"
    );
}

#[test]
fn undecodable_catalog_without_checkpoint_fails_closed() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    engine.insert(&mut tx, payload(1)).unwrap();
    engine.commit(tx).unwrap();
    drop(engine);
    std::fs::write(temp.path().join("schema.redline"), b"not a schema").unwrap();

    let err = open_err(temp.path(), config());
    assert_eq!(
        err,
        Error::CorruptPage("catalog file is unreadable and the wal holds no catalog snapshot")
    );
}

#[test]
fn missing_catalog_with_checkpoint_fails_closed() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    engine.insert(&mut tx, payload(1)).unwrap();
    engine.commit(tx).unwrap();
    engine.checkpoint().unwrap();
    drop(engine);
    std::fs::remove_file(temp.path().join("schema.redline")).unwrap();

    let err = open_err(temp.path(), config());
    assert_eq!(
        err,
        Error::CorruptPage("catalog file is missing and the wal holds no catalog snapshot")
    );
}

#[test]
fn missing_control_and_wal_with_written_pages_fails_open() {
    let temp = TempDir::new().unwrap();
    let engine = Engine::create(temp.path(), config()).unwrap();
    let mut tx = engine.begin(Isolation::Snapshot).unwrap();
    engine.insert(&mut tx, payload(1)).unwrap();
    engine.commit(tx).unwrap();
    let checkpoint = engine.checkpoint().unwrap();
    drop(engine);

    // Nothing says a checkpoint ever ran, and no WAL is left, yet the page
    // file holds a page past LSN zero: the log that wrote it is lost.
    std::fs::remove_file(temp.path().join("CONTROL_A")).unwrap();
    std::fs::remove_file(
        temp.path()
            .join(format!("TX_STATUS_{:020}", checkpoint.generation)),
    )
    .unwrap();
    std::fs::remove_dir_all(temp.path().join("wal")).unwrap();
    let before = files_under(temp.path());

    let err = open_err(temp.path(), config());
    assert_eq!(
        err,
        Error::CorruptWal("no checkpoint covers pages the wal no longer holds")
    );
    assert_eq!(
        files_under(temp.path()),
        before,
        "a failed open changed files"
    );
}
