//! Process-wide counters for the Gate 0 comparison.
//!
//! Relaxed atomics. They do not change SQL results, page bytes, or
//! durability. Callers sample [`snapshot`] around a run and subtract
//! with [`ObserveSnapshot::since`].

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
    }
}

#[inline]
pub fn add_checksum_bytes(n: u64) {
    OBSERVE.checksum_bytes.fetch_add(n, Ordering::Relaxed);
}

#[inline]
pub fn add_rewrite_leaf() {
    OBSERVE.rewrite_leaf_calls.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn add_row_lock_probe() {
    OBSERVE.row_lock_probes.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn add_all_frames(frames: u64) {
    OBSERVE.all_frames_calls.fetch_add(1, Ordering::Relaxed);
    OBSERVE
        .all_frames_frames
        .fetch_add(frames, Ordering::Relaxed);
}

#[inline]
pub fn add_page_file_mutex_wait(wait: Duration) {
    let ns = u64::try_from(wait.as_nanos()).unwrap_or(u64::MAX);
    OBSERVE
        .page_file_mutex_wait_ns
        .fetch_add(ns, Ordering::Relaxed);
    OBSERVE
        .page_file_mutex_acquires
        .fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn add_relation_get() {
    OBSERVE.relation_gets.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn add_sql_row_decode() {
    OBSERVE.sql_row_decodes.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn add_join_prefix_clone() {
    OBSERVE.join_prefix_clones.fetch_add(1, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::format::{Page, PageId, PageKind, RelId};

    use super::*;

    #[test]
    fn each_counter_moves_and_a_second_call_adds() {
        let before = snapshot();
        add_checksum_bytes(10);
        add_rewrite_leaf();
        add_row_lock_probe();
        add_all_frames(3);
        add_page_file_mutex_wait(Duration::from_nanos(15));
        add_relation_get();
        add_sql_row_decode();
        add_join_prefix_clone();
        let mid = snapshot();
        assert!(mid.checksum_bytes >= before.checksum_bytes + 10);
        assert!(mid.rewrite_leaf_calls >= before.rewrite_leaf_calls + 1);
        assert!(mid.row_lock_probes >= before.row_lock_probes + 1);
        assert!(mid.all_frames_calls >= before.all_frames_calls + 1);
        assert!(mid.all_frames_frames >= before.all_frames_frames + 3);
        assert!(mid.page_file_mutex_wait_ns >= before.page_file_mutex_wait_ns + 15);
        assert!(mid.page_file_mutex_acquires >= before.page_file_mutex_acquires + 1);
        assert!(mid.relation_gets >= before.relation_gets + 1);
        assert!(mid.sql_row_decodes >= before.sql_row_decodes + 1);
        assert!(mid.join_prefix_clones >= before.join_prefix_clones + 1);

        add_checksum_bytes(5);
        add_rewrite_leaf();
        add_row_lock_probe();
        add_all_frames(1);
        add_page_file_mutex_wait(Duration::from_nanos(1));
        add_relation_get();
        add_sql_row_decode();
        add_join_prefix_clone();
        let after = snapshot();
        assert!(after.checksum_bytes >= mid.checksum_bytes + 5);
        assert!(after.rewrite_leaf_calls >= mid.rewrite_leaf_calls + 1);
        assert!(after.row_lock_probes >= mid.row_lock_probes + 1);
        assert!(after.all_frames_calls >= mid.all_frames_calls + 1);
        assert!(after.all_frames_frames >= mid.all_frames_frames + 1);
        assert!(after.page_file_mutex_wait_ns >= mid.page_file_mutex_wait_ns + 1);
        assert!(after.page_file_mutex_acquires >= mid.page_file_mutex_acquires + 1);
        assert!(after.relation_gets >= mid.relation_gets + 1);
        assert!(after.sql_row_decodes >= mid.sql_row_decodes + 1);
        assert!(after.join_prefix_clones >= mid.join_prefix_clones + 1);
        let delta = after.since(before);
        assert!(delta.checksum_bytes >= 15);
        assert!(delta.rewrite_leaf_calls >= 2);
    }

    #[test]
    fn refresh_checksum_counts_page_bytes() {
        let before = snapshot();
        let _page = Page::new(512, PageKind::Heap, PageId(1), RelId(1)).expect("page");
        let mid = snapshot();
        assert!(mid.checksum_bytes >= before.checksum_bytes + 512);
        let _again = Page::new(512, PageKind::Heap, PageId(2), RelId(1)).expect("page");
        let after = snapshot();
        assert!(after.checksum_bytes >= mid.checksum_bytes + 512);
    }
}
