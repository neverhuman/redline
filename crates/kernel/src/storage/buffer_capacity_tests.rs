//! A failed concurrent load can free capacity while the clock examines frames.

use super::*;

#[test]
fn room_freed_by_failed_load_avoids_pressure_relief() {
    let dir = tempfile::tempdir().expect("temp dir");
    let file =
        Arc::new(PageFile::create(dir.path().join("data.redline"), 4096).expect("page file"));
    let pool = BufferPool::new(file, 1).expect("pool");
    let inner = Arc::clone(&pool.inner);
    let page_id = PageId(1);
    let loading = Arc::new(FrameEntry {
        state: Mutex::new(FrameState {
            page: None,
            pin_count: 1,
            dirty: false,
            usage_count: 1,
            write_in_progress: false,
            load_failed: false,
        }),
        ready: Condvar::new(),
    });
    let held = loading.state.lock().expect("loading frame lock");
    assert!(
        inner
            .try_insert_loading_frame(page_id, Arc::clone(&loading))
            .expect("insert loading frame")
    );

    std::thread::scope(|scope| {
        let worker = scope.spawn(|| inner.ensure_capacity());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while inner.eviction.try_lock().is_ok() {
            assert!(
                std::time::Instant::now() < deadline,
                "clock pass did not start"
            );
            std::thread::yield_now();
        }

        // Model the failed loader's shard removal while the clock pass is
        // waiting for this frame lock. The production removal makes the
        // same resident decrement without taking the eviction mutex.
        let removed = inner.shards[inner.shard_idx(page_id)]
            .lock()
            .expect("shard lock")
            .remove(&page_id)
            .expect("loading frame present");
        assert!(Arc::ptr_eq(&removed, &loading));
        inner.resident.fetch_sub(1, Ordering::Relaxed);
        drop(held);

        worker
            .join()
            .expect("clock worker")
            .expect("capacity became available without a checkpoint");
    });
    assert_eq!(pool.resident_pages(), 0);
    assert_eq!(pool.stats().pressure_checkpoints, 0);
}
