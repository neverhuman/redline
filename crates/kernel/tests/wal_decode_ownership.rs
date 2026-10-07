//! Observe the public scanner's bounded temporary ownership without timers or
//! allocator-address reuse assumptions. Only this test binary uses the recorder.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use redlinedb_kernel::format::{Lsn, TxId};
use redlinedb_kernel::wal::{WAL_HEADER_LEN, WalConfig, WalReader, WalRecord, WalRecordKind};
use tempfile::TempDir;

const LIMIT: usize = 512 << 10;
const MAX_EVENTS: usize = 512;

#[derive(Clone, Copy, Default)]
struct Event {
    pointer: usize,
    size: usize,
    allocated: bool,
}

#[derive(Clone, Copy)]
struct Trace {
    enabled: bool,
    overflow: bool,
    count: usize,
    events: [Event; MAX_EVENTS],
}

const EMPTY: Trace = Trace {
    enabled: false,
    overflow: false,
    count: 0,
    events: [Event {
        pointer: 0,
        size: 0,
        allocated: false,
    }; MAX_EVENTS],
};

thread_local! {
    // A const TLS Cell never allocates or locks inside the allocator callback.
    static TRACE: Cell<Trace> = const { Cell::new(EMPTY) };
}

fn observe(pointer: *mut u8, size: usize, allocated: bool) {
    if pointer.is_null() {
        return;
    }
    let _ = TRACE.try_with(|cell| {
        let mut trace = cell.get();
        if !trace.enabled {
            return;
        }
        if trace.count == MAX_EVENTS {
            trace.overflow = true;
        } else {
            trace.events[trace.count] = Event {
                pointer: pointer as usize,
                size,
                allocated,
            };
            trace.count += 1;
        }
        cell.set(trace);
    });
}

struct Recorder;

// SAFETY: every request is forwarded unchanged to System. The recorder neither
// dereferences nor retains ownership of pointers, and its fixed TLS storage
// performs no allocation, locking or unwinding in callbacks.
unsafe impl GlobalAlloc for Recorder {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies the valid allocation layout required by GlobalAlloc.
        let pointer = unsafe { System.alloc(layout) };
        observe(pointer, layout.size(), true);
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies the valid allocation layout required by GlobalAlloc.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        observe(pointer, layout.size(), true);
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        observe(pointer, layout.size(), false);
        // SAFETY: the caller supplies a live allocation and its original layout.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: the caller supplies a live allocation, original layout and valid new size.
        let resized = unsafe { System.realloc(pointer, layout, size) };
        if !resized.is_null() {
            observe(pointer, layout.size(), false);
            observe(resized, size, true);
        }
        resized
    }
}

#[global_allocator]
static ALLOCATOR: Recorder = Recorder;

fn capture<T>(operation: impl FnOnce() -> T) -> (T, Trace) {
    struct Disable;
    impl Drop for Disable {
        fn drop(&mut self) {
            TRACE.with(|cell| {
                let mut trace = cell.get();
                trace.enabled = false;
                cell.set(trace);
            });
        }
    }
    TRACE.with(|cell| {
        cell.set(Trace {
            enabled: true,
            ..EMPTY
        })
    });
    let guard = Disable;
    let result = operation();
    drop(guard);
    let trace = TRACE.with(Cell::get);
    assert!(!trace.overflow, "allocation recorder overflowed");
    (result, trace)
}

fn write_records(lengths: &[usize]) -> (TempDir, Vec<u8>, Vec<WalRecord>) {
    let temp = TempDir::new().expect("temp dir");
    let mut encoded = Vec::new();
    let mut records = Vec::new();
    let mut previous = Lsn::ZERO;
    for &length in lengths {
        let record = WalRecord {
            lsn: Lsn(encoded.len() as u64),
            prev_lsn: previous,
            tx_id: TxId(7),
            kind: WalRecordKind::PageDelta,
            payload: vec![23; length - WAL_HEADER_LEN],
        };
        encoded.extend(record.encode().expect("encode"));
        previous = record.lsn;
        records.push(record);
    }
    std::fs::write(temp.path().join(format!("{:020}.wal", 1)), &encoded).expect("write");
    (temp, encoded, records)
}

fn scanner(temp: &TempDir) -> WalReader {
    WalReader::new(
        temp.path(),
        WalConfig {
            segment_bytes: 4 << 20,
            ..WalConfig::default()
        },
    )
}

fn lifetimes(trace: &Trace, size: usize) -> Vec<(usize, usize)> {
    let events = &trace.events[..trace.count];
    let allocations: Vec<_> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| e.allocated && e.size == size)
        .collect();
    allocations
        .into_iter()
        .map(|(start, allocation)| {
            let end = events
                .iter()
                .enumerate()
                .skip(start + 1)
                .find(|(_, e)| !e.allocated && e.pointer == allocation.pointer && e.size == size)
                .expect("temporary encoded allocation must be freed")
                .0;
            (start, end)
        })
        .collect()
}

fn lifetime(trace: &Trace, size: usize) -> (usize, usize) {
    let found = lifetimes(trace, size);
    assert_eq!(
        found.len(),
        1,
        "one temporary encoded allocation of {size} bytes required"
    );
    found[0]
}

#[test]
fn ordinary_encoded_buffers_survive_decode_and_push_but_not_the_next_record() {
    let sizes = [305, LIMIT, LIMIT + 1, 337];
    let (temp, encoded, expected) = write_records(&sizes);
    let mut reader = scanner(&temp);
    let (report, trace) = capture(|| reader.scan_report().expect("scan"));
    assert_eq!(report.records, expected);
    assert_eq!(report.valid_end_lsn, Lsn(encoded.len() as u64));
    assert!(!report.torn_tail);
    assert_eq!(
        std::fs::read(temp.path().join(format!("{:020}.wal", 1))).expect("read"),
        encoded
    );
    let events = &trace.events[..trace.count];
    let (first, freed) = lifetime(&trace, sizes[0]);
    for pointer in [
        report.records[0].payload.as_ptr() as usize,
        report.records.as_ptr() as usize,
    ] {
        let allocated = events
            .iter()
            .rposition(|e| e.allocated && e.pointer == pointer)
            .expect("decoded payload/record vector allocation");
        assert!(
            first < allocated && allocated < freed,
            "encoded owner must survive decode and records.push"
        );
    }
    let (boundary, boundary_freed) = lifetime(&trace, LIMIT);
    let (last, _) = lifetime(&trace, sizes[3]);
    let oversized_payload = events
        .iter()
        .rposition(|e| e.allocated && e.pointer == report.records[2].payload.as_ptr() as usize)
        .expect("oversized decoded payload allocation");
    assert!(
        freed < boundary && boundary_freed < oversized_payload && oversized_payload < last,
        "temporary must not survive the next record"
    );
    assert!(
        !events.iter().any(|e| e.allocated && e.size == LIMIT + 1),
        "oversized encoded bytes must remain borrowed"
    );
}

#[test]
fn rejected_crc_and_link_paths_free_the_temporary_encoded_buffer() {
    for corrupt_crc in [true, false] {
        // The window is 654 bytes, distinct from either encoded allocation.
        let (temp, mut encoded, mut records) = write_records(&[305, 349]);
        if corrupt_crc {
            encoded[305 + WAL_HEADER_LEN] ^= 1;
        } else {
            records[0].prev_lsn = Lsn(1);
            encoded = records[0].encode().expect("encode invalid link");
            encoded.extend(records[1].encode().expect("encode following record"));
        }
        let path = temp.path().join(format!("{:020}.wal", 1));
        std::fs::write(&path, &encoded).expect("write invalid record");
        let mut reader = scanner(&temp);
        let (result, trace) = capture(|| reader.scan_report());
        lifetime(&trace, if corrupt_crc { 349 } else { 305 });
        if corrupt_crc {
            let report = result.expect("final CRC failure is a torn tail");
            assert!(report.torn_tail);
            assert_eq!(report.records, records[..1]);
        } else {
            assert!(result.is_err(), "invalid predecessor must fail");
        }
        assert_eq!(std::fs::read(path).expect("read"), encoded);
    }
}
