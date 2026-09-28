//! The advisory prefetch worker never runs a pressure checkpoint.
//!
//! A checkpoint on the worker thread upgrades the pool's weak reference to
//! the engine. If the application drops the engine meanwhile, the worker
//! holds the last reference, and dropping it drops the pool, whose `Drop`
//! joins the worker from itself: a panic, and an abort under the release
//! profile. A prefetch whose load would need a checkpoint is dropped
//! instead, as any other failed prefetch is.

use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

use super::BufferPool;
use crate::format::{Lsn, PageKind, RelId};
use crate::storage::{PageFile, PagePressureRelief};
use crate::telemetry::Phase11Counters;

const PAGE_SIZE: usize = 4096;

/// Records the name of every thread that asked for a checkpoint, and runs
/// none.
#[derive(Default)]
struct RecordingRelief {
    asked_by: Mutex<Vec<Option<String>>>,
}

impl PagePressureRelief for RecordingRelief {
    fn relieve_page_pressure(&self) -> crate::Result<bool> {
        self.asked_by
            .lock()
            .expect("relief log poisoned")
            .push(thread::current().name().map(str::to_owned));
        Ok(false)
    }
}

#[test]
fn the_prefetch_worker_never_asks_for_a_pressure_checkpoint() {
    let temp = tempfile::tempdir().expect("tempdir");
    let file =
        Arc::new(PageFile::create(temp.path().join("data.redline"), PAGE_SIZE).expect("file"));
    let pool = BufferPool::new(file, 1).expect("pool");
    // Page `cold` is on disk and not resident; the only frame holds a page
    // with a logged change, which only a checkpoint may write.
    let cold = pool.allocate(PageKind::Heap, RelId(1)).expect("allocate");
    let cold_id = cold.page_id();
    cold.mark_dirty(Lsn(1)).expect("dirty");
    drop(cold);
    pool.flush_page(cold_id, Lsn(1)).expect("flush");
    let logged = pool.allocate(PageKind::Heap, RelId(1)).expect("allocate");
    logged.mark_dirty(Lsn(10)).expect("dirty");
    drop(logged);

    let relief = Arc::new(RecordingRelief::default());
    let weak: Weak<dyn PagePressureRelief> =
        Arc::downgrade(&relief) as Weak<dyn PagePressureRelief>;
    pool.attach_pressure_relief(weak).expect("attach");

    // The worker takes one hint at a time, so once the queue is empty the
    // first hint's load has finished.
    let counters = Phase11Counters::new();
    pool.try_prefetch(cold_id, &counters);
    pool.try_prefetch(cold_id, &counters);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !pool.prefetch_queue.is_empty() {
        assert!(Instant::now() < deadline, "the prefetch worker never ran");
        thread::sleep(Duration::from_millis(5));
    }
    let asked_by = relief.asked_by.lock().expect("relief log").clone();
    assert!(
        asked_by
            .iter()
            .all(|name| name.as_deref() != Some("redlinedb-prefetch")),
        "the prefetch worker asked for a checkpoint: {asked_by:?}"
    );

    // A foreground load still asks.
    assert!(pool.pin(cold_id).is_err());
    assert!(
        !relief.asked_by.lock().expect("relief log").is_empty(),
        "a foreground pin did not ask for a checkpoint"
    );
}
