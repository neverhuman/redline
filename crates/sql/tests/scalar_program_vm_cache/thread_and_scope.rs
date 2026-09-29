//! Cache lifetime: one cache per OS thread, and `with_program_cache_scope`
//! clearing it on entry so a scope keeps only its own entries.

use super::*;

#[test]
fn cache_thread_local_isolation() {
    let _g = serial_guard();
    // The thread-local cache is per-OS-thread by definition. We verify
    // by populating a parent-thread cache, then asserting a freshly-
    // spawned worker thread sees an empty thread-local. The counters
    // are process-wide (AtomicU64) so they DO leak across threads;
    // that is intentional and asserted separately.
    clear_program_cache();
    reset_program_cache_counters();

    let expr = parse_expr("100 + 1");
    let ctx = empty_ctx();
    with_program_cache(|cache| {
        let _ = cache.get_or_compile(&expr, &ctx).expect("compile ok");
    });
    assert_eq!(program_cache_len(), 1, "parent thread populated cache");

    let handle = std::thread::spawn(|| {
        // Worker thread: its thread-local is independent. Must start
        // empty regardless of parent activity.
        assert_eq!(
            program_cache_len(),
            0,
            "worker thread must NOT share parent's cache entries",
        );
        // Populate the worker cache; parent must not see this entry.
        let expr = parse_expr("200 + 2");
        let ctx = empty_ctx();
        with_program_cache(|cache| {
            let _ = cache.get_or_compile(&expr, &ctx).expect("compile ok");
        });
        program_cache_len()
    });
    let worker_size = handle.join().expect("worker thread joined");
    assert_eq!(worker_size, 1, "worker thread populated its own cache");
    assert_eq!(
        program_cache_len(),
        1,
        "parent thread cache unaffected by worker thread",
    );
}

#[test]
fn cache_size_bounded_by_per_query_scope() {
    let _g = serial_guard();
    // `with_program_cache_scope` clears the per-thread cache on
    // entry. Successive scopes start fresh, so the cache never grows
    // beyond a single query's working set even when many queries run
    // serially on the same thread.
    clear_program_cache();

    // Scope 1 populates the cache with two entries.
    let scope_1_size = with_program_cache_scope(|| {
        let ctx = empty_ctx();
        let e1 = parse_expr("1 + 1");
        let e2 = parse_expr("2 * 2");
        with_program_cache(|c| {
            let _ = c.get_or_compile(&e1, &ctx).expect("compile ok");
            let _ = c.get_or_compile(&e2, &ctx).expect("compile ok");
        });
        program_cache_len()
    });
    assert_eq!(scope_1_size, 2, "scope 1 populated 2 entries");

    // Scope 2 must NOT see scope 1's entries — the scope guard
    // clears on entry.
    let scope_2_size = with_program_cache_scope(|| {
        // Cache is empty on scope entry regardless of scope 1's
        // contents.
        assert_eq!(
            program_cache_len(),
            0,
            "scope 2 must start with an empty cache",
        );
        let ctx = empty_ctx();
        let e3 = parse_expr("3 - 3");
        with_program_cache(|c| {
            let _ = c.get_or_compile(&e3, &ctx).expect("compile ok");
        });
        program_cache_len()
    });
    assert_eq!(
        scope_2_size, 1,
        "scope 2 holds only its own entries — bounded by per-query scope",
    );

    // A scope that produces N distinct expression shapes ends with
    // exactly N entries; the cache is not unbounded.
    let burst_size = with_program_cache_scope(|| {
        let ctx = empty_ctx();
        for i in 0..16i64 {
            let expr = parse_expr(&format!("{i} + 1"));
            with_program_cache(|c| {
                let _ = c.get_or_compile(&expr, &ctx).expect("compile ok");
            });
        }
        program_cache_len()
    });
    assert_eq!(
        burst_size, 16,
        "16 distinct shapes → 16 entries (cache grows linearly within a scope, not unboundedly across scopes)",
    );
}
