//! Process-wide counters for the Gate 0 comparison.
//!
//! Relaxed atomics. They do not change SQL results, page bytes, or
//! durability. Callers sample [`snapshot`] around a run and subtract
//! with [`ObserveSnapshot::since`].
//!
//! [`snapshot`] sums every thread, so a test that reads it while other
//! tests run in the same process counts their work too. Kernel unit tests
//! read `thread_snapshot` instead: test builds also count each event on the
//! thread that made it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ObserveSnapshot {
    pub checksum_bytes: u64,
    pub rewrite_leaf_calls: u64,
    pub row_lock_probes: u64,
    pub all_frames_calls: u64,
    pub all_frames_frames: u64,
    pub page_file_mutex_wait_ns: u64,
    pub page_file_mutex_acquires: u64,
    pub relation_gets: u64,
    pub sql_row_decodes: u64,
    pub join_prefix_clones: u64,
    /// Wake-ups of threads waiting on a buffer frame after a page load or write.
    pub frame_wakeups: u64,
    /// Successful buffer-pool page pins, including resident and cold loads.
    pub heap_page_pins: u64,
    /// Buffer-frame condition-variable notifications after a load or write.
    pub frame_notifies: u64,
    /// Row IDs copied from the relation directory into scan vectors.
    pub directory_entries_copied: u64,
    /// Signals sent by WAL producers to the writer condition variable.
    pub wal_writer_wakeups: u64,
}

impl ObserveSnapshot {
    pub fn since(self, earlier: Self) -> Self {
        Self {
            checksum_bytes: self.checksum_bytes.saturating_sub(earlier.checksum_bytes),
            rewrite_leaf_calls: self
                .rewrite_leaf_calls
                .saturating_sub(earlier.rewrite_leaf_calls),
            row_lock_probes: self.row_lock_probes.saturating_sub(earlier.row_lock_probes),
            all_frames_calls: self
                .all_frames_calls
                .saturating_sub(earlier.all_frames_calls),
            all_frames_frames: self
                .all_frames_frames
                .saturating_sub(earlier.all_frames_frames),
            page_file_mutex_wait_ns: self
                .page_file_mutex_wait_ns
                .saturating_sub(earlier.page_file_mutex_wait_ns),
            page_file_mutex_acquires: self
                .page_file_mutex_acquires
                .saturating_sub(earlier.page_file_mutex_acquires),
            relation_gets: self.relation_gets.saturating_sub(earlier.relation_gets),
            sql_row_decodes: self.sql_row_decodes.saturating_sub(earlier.sql_row_decodes),
            join_prefix_clones: self
                .join_prefix_clones
                .saturating_sub(earlier.join_prefix_clones),
            frame_wakeups: self.frame_wakeups.saturating_sub(earlier.frame_wakeups),
            heap_page_pins: self.heap_page_pins.saturating_sub(earlier.heap_page_pins),
            frame_notifies: self.frame_notifies.saturating_sub(earlier.frame_notifies),
            directory_entries_copied: self
                .directory_entries_copied
                .saturating_sub(earlier.directory_entries_copied),
            wal_writer_wakeups: self
                .wal_writer_wakeups
                .saturating_sub(earlier.wal_writer_wakeups),
        }
    }
}

struct ObserveCounters {
    checksum_bytes: AtomicU64,
    rewrite_leaf_calls: AtomicU64,
    row_lock_probes: AtomicU64,
    all_frames_calls: AtomicU64,
    all_frames_frames: AtomicU64,
    page_file_mutex_wait_ns: AtomicU64,
    page_file_mutex_acquires: AtomicU64,
    relation_gets: AtomicU64,
    sql_row_decodes: AtomicU64,
    join_prefix_clones: AtomicU64,
    frame_wakeups: AtomicU64,
    heap_page_pins: AtomicU64,
    frame_notifies: AtomicU64,
    directory_entries_copied: AtomicU64,
    wal_writer_wakeups: AtomicU64,
}

static OBSERVE: ObserveCounters = ObserveCounters {
    checksum_bytes: AtomicU64::new(0),
    rewrite_leaf_calls: AtomicU64::new(0),
    row_lock_probes: AtomicU64::new(0),
    all_frames_calls: AtomicU64::new(0),
    all_frames_frames: AtomicU64::new(0),
    page_file_mutex_wait_ns: AtomicU64::new(0),
    page_file_mutex_acquires: AtomicU64::new(0),
    relation_gets: AtomicU64::new(0),
    sql_row_decodes: AtomicU64::new(0),
    join_prefix_clones: AtomicU64::new(0),
    frame_wakeups: AtomicU64::new(0),
    heap_page_pins: AtomicU64::new(0),
    frame_notifies: AtomicU64::new(0),
    directory_entries_copied: AtomicU64::new(0),
    wal_writer_wakeups: AtomicU64::new(0),
};

pub fn snapshot() -> ObserveSnapshot {
    ObserveSnapshot {
        checksum_bytes: OBSERVE.checksum_bytes.load(Ordering::Relaxed),
        rewrite_leaf_calls: OBSERVE.rewrite_leaf_calls.load(Ordering::Relaxed),
        row_lock_probes: OBSERVE.row_lock_probes.load(Ordering::Relaxed),
        all_frames_calls: OBSERVE.all_frames_calls.load(Ordering::Relaxed),
        all_frames_frames: OBSERVE.all_frames_frames.load(Ordering::Relaxed),
        page_file_mutex_wait_ns: OBSERVE.page_file_mutex_wait_ns.load(Ordering::Relaxed),
        page_file_mutex_acquires: OBSERVE.page_file_mutex_acquires.load(Ordering::Relaxed),
        relation_gets: OBSERVE.relation_gets.load(Ordering::Relaxed),
        sql_row_decodes: OBSERVE.sql_row_decodes.load(Ordering::Relaxed),
        join_prefix_clones: OBSERVE.join_prefix_clones.load(Ordering::Relaxed),
        frame_wakeups: OBSERVE.frame_wakeups.load(Ordering::Relaxed),
        heap_page_pins: OBSERVE.heap_page_pins.load(Ordering::Relaxed),
        frame_notifies: OBSERVE.frame_notifies.load(Ordering::Relaxed),
        directory_entries_copied: OBSERVE.directory_entries_copied.load(Ordering::Relaxed),
        wal_writer_wakeups: OBSERVE.wal_writer_wakeups.load(Ordering::Relaxed),
    }
}

/// Which [`ObserveSnapshot`] field a global counter mirrors.
type Field = fn(&mut ObserveSnapshot) -> &mut u64;

#[inline]
fn add(global: &AtomicU64, field: Field, n: u64) {
    global.fetch_add(n, Ordering::Relaxed);
    #[cfg(test)]
    this_thread::add(field, n);
    #[cfg(not(test))]
    let _ = field;
}

#[inline]
pub fn add_checksum_bytes(n: u64) {
    add(&OBSERVE.checksum_bytes, |s| &mut s.checksum_bytes, n);
}

#[inline]
pub fn add_rewrite_leaf() {
    add(
        &OBSERVE.rewrite_leaf_calls,
        |s| &mut s.rewrite_leaf_calls,
        1,
    );
}

#[inline]
pub fn add_row_lock_probe() {
    add(&OBSERVE.row_lock_probes, |s| &mut s.row_lock_probes, 1);
}

#[inline]
pub fn add_all_frames(frames: u64) {
    add(&OBSERVE.all_frames_calls, |s| &mut s.all_frames_calls, 1);
    add(
        &OBSERVE.all_frames_frames,
        |s| &mut s.all_frames_frames,
        frames,
    );
}

#[inline]
pub fn add_page_file_mutex_wait(wait: Duration) {
    let ns = u64::try_from(wait.as_nanos()).unwrap_or(u64::MAX);
    add(
        &OBSERVE.page_file_mutex_wait_ns,
        |s| &mut s.page_file_mutex_wait_ns,
        ns,
    );
    add(
        &OBSERVE.page_file_mutex_acquires,
        |s| &mut s.page_file_mutex_acquires,
        1,
    );
}

#[inline]
pub fn add_relation_get() {
    add(&OBSERVE.relation_gets, |s| &mut s.relation_gets, 1);
}

#[inline]
pub fn add_sql_row_decode() {
    add(&OBSERVE.sql_row_decodes, |s| &mut s.sql_row_decodes, 1);
}

#[inline]
pub fn add_frame_wakeup() {
    add(&OBSERVE.frame_wakeups, |s| &mut s.frame_wakeups, 1);
}

#[inline]
pub fn add_heap_page_pin() {
    add(&OBSERVE.heap_page_pins, |s| &mut s.heap_page_pins, 1);
}

#[inline]
pub fn add_frame_notify() {
    add(&OBSERVE.frame_notifies, |s| &mut s.frame_notifies, 1);
}

#[inline]
pub fn add_directory_entries_copied(n: u64) {
    add(
        &OBSERVE.directory_entries_copied,
        |s| &mut s.directory_entries_copied,
        n,
    );
}

#[inline]
pub fn add_wal_writer_wakeup() {
    add(
        &OBSERVE.wal_writer_wakeups,
        |s| &mut s.wal_writer_wakeups,
        1,
    );
}

#[inline]
pub fn add_join_prefix_clone() {
    add(
        &OBSERVE.join_prefix_clones,
        |s| &mut s.join_prefix_clones,
        1,
    );
}

/// Counts made on this thread since it started.
///
/// Test builds only. Tests run as threads of one process, so a test that
/// counts with [`snapshot`] also counts every test running beside it.
#[cfg(test)]
pub(crate) fn thread_snapshot() -> ObserveSnapshot {
    this_thread::snapshot()
}

#[cfg(test)]
mod this_thread {
    use std::cell::Cell;

    use super::{Field, ObserveSnapshot};

    thread_local! {
        static COUNTS: Cell<ObserveSnapshot> = const {
            Cell::new(ObserveSnapshot {
                checksum_bytes: 0,
                rewrite_leaf_calls: 0,
                row_lock_probes: 0,
                all_frames_calls: 0,
                all_frames_frames: 0,
                page_file_mutex_wait_ns: 0,
                page_file_mutex_acquires: 0,
                relation_gets: 0,
                sql_row_decodes: 0,
                join_prefix_clones: 0,
                frame_wakeups: 0,
                heap_page_pins: 0,
                frame_notifies: 0,
                directory_entries_copied: 0,
                wal_writer_wakeups: 0,
            })
        };
    }

    pub(super) fn add(field: Field, n: u64) {
        COUNTS.with(|cell| {
            let mut counts = cell.get();
            let slot = field(&mut counts);
            *slot = slot.wrapping_add(n);
            cell.set(counts);
        });
    }

    pub(super) fn snapshot() -> ObserveSnapshot {
        COUNTS.with(Cell::get)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::format::{Page, PageId, PageKind, RelId};

    use super::*;

    fn add_each(checksum: u64, frames: u64, wait_ns: u64) {
        add_checksum_bytes(checksum);
        add_rewrite_leaf();
        add_row_lock_probe();
        add_all_frames(frames);
        add_page_file_mutex_wait(Duration::from_nanos(wait_ns));
        add_relation_get();
        add_sql_row_decode();
        add_join_prefix_clone();
        add_frame_wakeup();
        add_heap_page_pin();
        add_frame_notify();
        add_directory_entries_copied(5);
        add_wal_writer_wakeup();
    }

    fn each(checksum: u64, frames: u64, wait_ns: u64) -> ObserveSnapshot {
        ObserveSnapshot {
            checksum_bytes: checksum,
            rewrite_leaf_calls: 1,
            row_lock_probes: 1,
            all_frames_calls: 1,
            all_frames_frames: frames,
            page_file_mutex_wait_ns: wait_ns,
            page_file_mutex_acquires: 1,
            relation_gets: 1,
            sql_row_decodes: 1,
            join_prefix_clones: 1,
            frame_wakeups: 1,
            heap_page_pins: 1,
            frame_notifies: 1,
            directory_entries_copied: 5,
            wal_writer_wakeups: 1,
        }
    }

    /// Other tests only add to the process-wide counters, so a process-wide
    /// delta holds at least what one thread counted.
    fn assert_covers(total: ObserveSnapshot, part: ObserveSnapshot) {
        let pairs = [
            (total.checksum_bytes, part.checksum_bytes),
            (total.rewrite_leaf_calls, part.rewrite_leaf_calls),
            (total.row_lock_probes, part.row_lock_probes),
            (total.all_frames_calls, part.all_frames_calls),
            (total.all_frames_frames, part.all_frames_frames),
            (total.page_file_mutex_wait_ns, part.page_file_mutex_wait_ns),
            (
                total.page_file_mutex_acquires,
                part.page_file_mutex_acquires,
            ),
            (total.relation_gets, part.relation_gets),
            (total.sql_row_decodes, part.sql_row_decodes),
            (total.join_prefix_clones, part.join_prefix_clones),
            (total.frame_wakeups, part.frame_wakeups),
            (total.heap_page_pins, part.heap_page_pins),
            (total.frame_notifies, part.frame_notifies),
            (
                total.directory_entries_copied,
                part.directory_entries_copied,
            ),
            (total.wal_writer_wakeups, part.wal_writer_wakeups),
        ];
        for (total_count, part_count) in pairs {
            assert!(total_count >= part_count, "{total:?} misses {part:?}");
        }
    }

    #[test]
    fn each_counter_moves_and_a_second_call_adds() {
        let process_before = snapshot();
        let before = thread_snapshot();
        add_each(10, 3, 15);
        let mid = thread_snapshot();
        assert_eq!(mid.since(before), each(10, 3, 15));

        add_each(5, 1, 1);
        let after = thread_snapshot();
        assert_eq!(after.since(mid), each(5, 1, 1));
        let delta = after.since(before);
        assert_eq!(delta.checksum_bytes, 15);
        assert_eq!(delta.rewrite_leaf_calls, 2);

        assert_covers(snapshot().since(process_before), delta);
    }

    #[test]
    fn thread_snapshot_ignores_other_threads() {
        let process_before = snapshot();
        let before = thread_snapshot();
        std::thread::spawn(|| add_each(7, 2, 9))
            .join()
            .expect("counting thread");
        assert_eq!(thread_snapshot().since(before), ObserveSnapshot::default());
        assert_covers(snapshot().since(process_before), each(7, 2, 9));
    }

    #[test]
    fn refresh_checksum_counts_page_bytes() {
        let before = thread_snapshot();
        let _page = Page::new(512, PageKind::Heap, PageId(1), RelId(1)).expect("page");
        let mid = thread_snapshot();
        assert_eq!(mid.since(before).checksum_bytes, 512);
        let _again = Page::new(512, PageKind::Heap, PageId(2), RelId(1)).expect("page");
        let after = thread_snapshot();
        assert_eq!(after.since(mid).checksum_bytes, 512);
    }
}
